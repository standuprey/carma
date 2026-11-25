//! The real capture backend: a PipeWire streaming source and registry-based
//! device enumeration, both behind the `interfaces::AudioSource` trait and the
//! small `Device` value type.
//!
//! A capture source is push-based at the PipeWire boundary (callbacks deliver
//! buffers on a real-time thread), but the `AudioSource` trait is pull-based
//! (`read(&mut [f32])`). We bridge the two with a bounded FIFO: the PipeWire
//! process callback appends interleaved f32 samples, and `read` drains them.
//! A dedicated PipeWire worker thread owns the main loop for its whole life so
//! nothing tied to the loop is ever moved between threads; a periodic timer
//! observes the shared stop flag and quits the loop when the source is dropped.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

use interfaces::{AudioSource, CHANNELS, SAMPLE_RATE};

use pipewire as pw;

/// Maximum samples buffered before the newest block overwrites the oldest.
/// Roughly a second of stereo audio at 48 kHz; keeps the worker bounded while
/// the consumer catches up. Underrun handling is refined in the robustness
/// ticket.
const RING_CAPACITY: usize = 48_000 * CHANNELS;

/// An enumerated capture source on the PipeWire graph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Device {
    /// Registry object id of the source node, used to target the stream.
    pub id: u32,
    /// Stable `node.name` (e.g. `alsa_input.pci-...`).
    pub name: String,
    /// Human-readable `node.description` (e.g. `Scarlett Solo USB`).
    pub description: String,
}

/// Errors produced by the PipeWire backend.
#[derive(Debug, thiserror::Error)]
pub enum AudioIoError {
    #[error("pipewire error: {0}")]
    PipeWire(String),
    #[error("timed out connecting to pipewire")]
    Timeout,
}

/// Lists the available capture (`Audio/Source`) nodes in the PipeWire registry.
///
/// Runs a short-lived main loop until the registry's initial burst has been
/// received; returns nothing once the daemon is unreachable (which surfaces as
/// [`AudioIoError::PipeWire`]).
/// Lists the available capture (`Audio/Source`) nodes in the PipeWire registry.
///
/// Runs a short-lived main loop until the registry's initial burst has been
/// received; returns nothing once the daemon is unreachable (which surfaces as
/// [`AudioIoError::PipeWire`]).
pub fn list_capture_devices() -> Result<Vec<Device>, AudioIoError> {
    list_nodes(move |props| props.get(*pw::keys::MEDIA_CLASS) == Some("Audio/Source"))
}

/// Collects every Node global in the registry that `filter` accepts.
fn list_nodes<F>(filter: F) -> Result<Vec<Device>, AudioIoError>
where
    F: Fn(&pw::spa::utils::dict::DictRef) -> bool + 'static,
{
    pw::init();
    let mainloop =
        pw::main_loop::MainLoopRc::new(None).map_err(|e| AudioIoError::PipeWire(e.to_string()))?;
    let context = pw::context::ContextRc::new(&mainloop, None)
        .map_err(|e| AudioIoError::PipeWire(e.to_string()))?;
    let core = context
        .connect_rc(None)
        .map_err(|e| AudioIoError::PipeWire(e.to_string()))?;
    let registry = core
        .get_registry_rc()
        .map_err(|e| AudioIoError::PipeWire(e.to_string()))?;

    let devices = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let devices_cb = std::rc::Rc::clone(&devices);
    let _registry_listener = registry
        .add_listener_local()
        .global(move |obj| {
            if obj.type_ != pw::types::ObjectType::Node {
                return;
            }
            let Some(props) = obj.props.as_ref() else {
                return;
            };
            if !filter(props) {
                return;
            }
            let name = props.get(*pw::keys::NODE_NAME).unwrap_or("").to_string();
            let description = props
                .get(*pw::keys::NODE_DESCRIPTION)
                .unwrap_or(&name)
                .to_string();
            devices_cb.borrow_mut().push(Device {
                id: obj.id,
                name,
                description,
            });
        })
        .register();

    let mainloop_weak = mainloop.downgrade();
    let _core_listener = core
        .add_listener_local()
        .done(move |_id, _seq| {
            if let Some(ml) = mainloop_weak.upgrade() {
                ml.quit();
            }
        })
        .register();
    let _ = core.sync(0);

    mainloop.run();

    // The registry/core listeners still hold `Rc` clones, so clone the collected
    // vector out and let the call return normally.
    let out = devices.borrow().clone();
    Ok(out)
}

/// Finds a capture source from a user-supplied selector.
///
/// Matches by substring against `node.description` first (the friendly name a
/// user would type), falling back to an exact `node.name` match.
pub fn find_capture_device(selector: &str) -> Result<Option<Device>, AudioIoError> {
    let devices = list_capture_devices()?;
    if let Some(device) = devices.iter().find(|d| d.description.contains(selector)) {
        return Ok(Some(device.clone()));
    }
    Ok(devices.into_iter().find(|d| d.name == selector))
}

/// Resolves a selector to any Node by exact `node.name`, falling back to a
/// substring match against `node.description`. Used to connect to non-device
/// nodes (e.g. a loopback's output) for automated capture tests.
pub fn find_node(selector: &str) -> Result<Option<Device>, AudioIoError> {
    let nodes = list_nodes(|_| true)?;
    if let Some(device) = nodes.iter().find(|d| d.name == selector) {
        return Ok(Some(device.clone()));
    }
    Ok(nodes.into_iter().find(|d| d.description.contains(selector)))
}

/// A real capture source over libpipewire.
///
/// Owns a PipeWire worker thread that streams from the chosen device node into
/// a bounded FIFO; `read` pops interleaved 48 kHz stereo f32 frames.
pub struct PipeWireAudioSource {
    fifo: Arc<Mutex<VecDeque<f32>>>,
    channels: usize,
    sample_rate: u32,
    stop: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl PipeWireAudioSource {
    /// Connects to a capture node `device` and starts streaming. Fails if the
    /// PipeWire daemon is unreachable or the connection cannot be established.
    pub fn new(device: &Device) -> Result<Self, AudioIoError> {
        let fifo = Arc::new(Mutex::new(VecDeque::with_capacity(RING_CAPACITY)));
        let stop = Arc::new(AtomicBool::new(false));
        let (ready_tx, ready_rx) = mpsc::channel::<Result<(), String>>();

        let node_id = device.id;
        let fifo_thread = Arc::clone(&fifo);
        let stop_thread = Arc::clone(&stop);

        let handle = std::thread::Builder::new()
            .name("carma-pipewire".into())
            .spawn(move || {
                let notify = |res: Result<(), String>| {
                    let _ = ready_tx.send(res);
                };

                pw::init();
                let mainloop = match pw::main_loop::MainLoopRc::new(None) {
                    Ok(ml) => ml,
                    Err(e) => {
                        notify(Err(e.to_string()));
                        return;
                    }
                };
                let context = match pw::context::ContextRc::new(&mainloop, None) {
                    Ok(c) => c,
                    Err(e) => {
                        notify(Err(e.to_string()));
                        return;
                    }
                };
                let core = match context.connect_rc(None) {
                    Ok(c) => c,
                    Err(e) => {
                        notify(Err(e.to_string()));
                        return;
                    }
                };

                let props = pw::properties::properties! {
                    *pw::keys::MEDIA_TYPE => "Audio",
                    *pw::keys::MEDIA_CATEGORY => "Capture",
                    *pw::keys::MEDIA_ROLE => "Music",
                };
                let stream = match pw::stream::StreamBox::new(&core, "carma-capture", props) {
                    Ok(s) => s,
                    Err(e) => {
                        notify(Err(e.to_string()));
                        return;
                    }
                };

                let fifo_cb = Arc::clone(&fifo_thread);
                let _listener = stream
                    .add_local_listener_with_user_data(())
                    .process(move |stream, _| {
                        let Some(mut buffer) = stream.dequeue_buffer() else {
                            return;
                        };
                        let datas = buffer.datas_mut();
                        if datas.is_empty() {
                            return;
                        }
                        let data = &mut datas[0];
                        let n_samples = data.chunk().size() as usize / std::mem::size_of::<f32>();
                        let Some(bytes) = data.data() else {
                            return;
                        };
                        let mut fifo = fifo_cb.lock().unwrap();
                        for i in 0..n_samples {
                            let start = i * std::mem::size_of::<f32>();
                            let end = start + std::mem::size_of::<f32>();
                            let arr: [u8; 4] = bytes[start..end].try_into().unwrap();
                            let sample = f32::from_le_bytes(arr);
                            if fifo.len() >= RING_CAPACITY {
                                fifo.pop_front();
                            }
                            fifo.push_back(sample);
                        }
                    })
                    .register();

                let mut audio_info = pw::spa::param::audio::AudioInfoRaw::new();
                audio_info.set_format(pw::spa::param::audio::AudioFormat::F32LE);
                audio_info.set_rate(SAMPLE_RATE);
                audio_info.set_channels(CHANNELS as u32);
                let format_object = pw::spa::pod::Object {
                    type_: pw::spa::utils::SpaTypes::ObjectParamFormat.as_raw(),
                    id: pw::spa::param::ParamType::EnumFormat.as_raw(),
                    properties: audio_info.into(),
                };
                let serialized: Vec<u8> = match pw::spa::pod::serialize::PodSerializer::serialize(
                    std::io::Cursor::new(Vec::new()),
                    &pw::spa::pod::Value::Object(format_object),
                ) {
                    Ok((cursor, _)) => cursor.into_inner(),
                    Err(e) => {
                        notify(Err(e.to_string()));
                        return;
                    }
                };
                let Some(format_pod) = pw::spa::pod::Pod::from_bytes(&serialized) else {
                    notify(Err("failed to encode capture format".to_string()));
                    return;
                };
                let mut params = [format_pod];

                if let Err(e) = stream.connect(
                    pw::spa::utils::Direction::Input,
                    Some(node_id),
                    pw::stream::StreamFlags::AUTOCONNECT
                        | pw::stream::StreamFlags::MAP_BUFFERS
                        | pw::stream::StreamFlags::RT_PROCESS,
                    &mut params,
                ) {
                    notify(Err(e.to_string()));
                    return;
                }

                // Periodic timer observes the stop flag and quits the loop so
                // the worker can end when the source is dropped.
                let stop_cb = Arc::clone(&stop_thread);
                let loop_quit = mainloop.clone();
                let loop_ref = mainloop.loop_();
                let _timer = loop_ref.add_timer(move |_| {
                    if stop_cb.load(Ordering::Acquire) {
                        loop_quit.quit();
                    }
                });
                let _ = _timer.update_timer(
                    Some(Duration::from_millis(50)),
                    Some(Duration::from_millis(50)),
                );

                notify(Ok(()));
                mainloop.run();
            })
            .map_err(|e| AudioIoError::PipeWire(e.to_string()))?;

        match ready_rx.recv_timeout(Duration::from_secs(5)) {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                stop.store(true, Ordering::Release);
                let _ = handle.join();
                return Err(AudioIoError::PipeWire(e));
            }
            Err(_) => {
                stop.store(true, Ordering::Release);
                let _ = handle.join();
                return Err(AudioIoError::Timeout);
            }
        }

        Ok(Self {
            fifo,
            channels: CHANNELS,
            sample_rate: SAMPLE_RATE,
            stop,
            handle: Some(handle),
        })
    }
}

impl AudioSource for PipeWireAudioSource {
    fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    fn channels(&self) -> usize {
        self.channels
    }

    fn read(&mut self, out: &mut [f32]) -> usize {
        let mut fifo = self.fifo.lock().unwrap();
        let max_frames = out.len() / self.channels;
        let available = fifo.len() / self.channels;
        let frames = available.min(max_frames);
        if frames == 0 {
            // Backpressure: no frames ready yet. Pause so the consumer does not
            // busy-loop (mirrors the fake source's pacing behaviour).
            std::thread::sleep(Duration::from_millis(1));
            return 0;
        }
        for slot in out[..frames * self.channels].iter_mut() {
            *slot = fifo.pop_front().unwrap();
        }
        frames
    }
}

impl Drop for PipeWireAudioSource {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    /// Best-effort probe: does the PipeWire daemon answer on this host?
    fn pipewire_available() -> bool {
        list_capture_devices().is_ok()
    }

    /// Writes a short WAV (RIFF/WAVE, S16LE, stereo, 48 kHz) carrying a 1 kHz
    /// tone to `path`, so the loopback test feeds a known, reproducible signal
    /// (no human speech, no external mic needed).
    fn write_tone_wav(path: &std::path::Path, seconds: u32) {
        let sample_rate = 48_000u32;
        let channels = 2u16;
        let total_samples = (sample_rate as u64 * seconds as u64) as usize;
        let data_len = (total_samples * channels as usize * 2) as u32;
        let mut data = Vec::with_capacity(total_samples * channels as usize * 2);
        for i in 0..total_samples {
            let phase = 2.0 * std::f64::consts::PI * 1000.0 * i as f64 / sample_rate as f64;
            let v = 0.4 * phase.sin() as f32;
            let s = (v * 32767.0) as i16;
            for _ in 0..channels {
                data.extend_from_slice(&s.to_le_bytes());
            }
        }
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(36 + data_len).to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16u32.to_le_bytes()); // fmt chunk size
        bytes.extend_from_slice(&1u16.to_le_bytes()); // PCM
        bytes.extend_from_slice(&channels.to_le_bytes());
        bytes.extend_from_slice(&sample_rate.to_le_bytes());
        bytes.extend_from_slice(&(sample_rate * channels as u32 * 2).to_le_bytes()); // byte rate
        bytes.extend_from_slice(&(channels * 2).to_le_bytes()); // block align
        bytes.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&data_len.to_le_bytes());
        bytes.extend_from_slice(&data);
        std::fs::write(path, bytes).unwrap();
    }

    fn command_available(program: &str) -> bool {
        std::process::Command::new(program)
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success() || !s.code().is_some_and(|c| c == u8::MAX as i32))
            .unwrap_or(false)
    }

    #[test]
    fn enumerates_capture_sources() {
        if !pipewire_available() {
            eprintln!("skip: PipeWire daemon unreachable in this environment");
            return;
        }
        let devices = list_capture_devices().expect("enumeration must not error");
        assert!(
            !devices.is_empty(),
            "expected at least one Audio/Source node on a live PipeWire graph"
        );
        for d in &devices {
            assert!(!d.name.is_empty(), "capture device must have a node name");
            assert!(
                !d.description.is_empty(),
                "capture device needs a description"
            );
        }
    }

    #[test]
    fn resolves_capture_device_by_description_and_name() {
        if !pipewire_available() {
            eprintln!("skip: PipeWire daemon unreachable in this environment");
            return;
        }
        let devices = list_capture_devices().unwrap();
        let first = &devices[0];
        // Any leading slice of a real device's description must round-trip.
        let prefix = &first.description[..first.description.len().min(4)];
        let by_desc = find_capture_device(prefix).expect("lookup must not error");
        assert!(
            by_desc.is_some(),
            "description substring should resolve a device"
        );
        let by_name = find_capture_device(&first.name).expect("lookup must not error");
        assert_eq!(by_name.as_ref().map(|d| &d.name), Some(&first.name));
        // An empty/unrecognised selector resolves to nothing.
        assert!(find_capture_device("definitely-not-a-device-xyz")
            .unwrap()
            .is_none());
    }

    #[test]
    fn captures_frames_from_loopback_signal() {
        if !pipewire_available() {
            eprintln!("skip: PipeWire daemon unreachable in this environment");
            return;
        }
        for prog in ["pw-cli", "pw-loopback", "pw-cat"] {
            if !command_available(prog) {
                eprintln!("skip: required tool '{prog}' not found");
                return;
            }
        }

        let uniq = std::process::id();
        let sink_name = format!("carmnull_{uniq}");
        let loop_name = format!("carmloop_{uniq}");
        let sink_node = format!("input.{loop_name}");
        let source_node = format!("output.{loop_name}");

        // Leave a deterministic target to monitor and feed.
        let sink_created = std::process::Command::new("pw-cli")
            .args([
                "create-node",
                "adapter",
                &format!(
                    "{{ factory.name=support.null-audio-sink, node.name={sink_name}, \
                     media.class=Audio/Sink, audio.position=[FL FR] }}"
                ),
            ])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if !sink_created {
            eprintln!("skip: could not create a null-audio-sink node");
            return;
        }

        let mut loopback = std::process::Command::new("pw-loopback")
            .args(["--playback", &sink_name, "-n", &loop_name])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("pw-loopback must spawn");

        // Wait for the loopback's source node to appear.
        let source = {
            let mut found = None;
            for _ in 0..30 {
                std::thread::sleep(Duration::from_millis(100));
                if let Ok(Some(device)) = find_node(&source_node) {
                    found = Some(device);
                    break;
                }
            }
            found
        };
        let Some(source) = source else {
            eprintln!("skip: loopback source node never appeared");
            let _ = loopback.kill();
            let _ = loopback.wait();
            return;
        };

        // Feed the known tone into the loopback sink's input side.
        let tone_path = std::env::temp_dir().join(format!("carma_tone_{uniq}.wav"));
        write_tone_wav(&tone_path, 5);
        let player = std::process::Command::new("pw-cat")
            .args([
                "--playback",
                "--target",
                &sink_node,
                tone_path.to_str().unwrap(),
            ])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("pw-cat must spawn");

        let mut ausrc = PipeWireAudioSource::new(&source).expect("source must connect");
        assert_eq!(ausrc.sample_rate(), 48_000);
        assert_eq!(ausrc.channels(), 2);

        // Receive at least one non-empty frame block from the loopback.
        let mut buf = [0.0f32; 1024 * 2];
        let mut received = 0usize;
        let deadline = std::time::Instant::now() + Duration::from_secs(4);
        while received == 0 && std::time::Instant::now() < deadline {
            received = ausrc.read(&mut buf);
        }
        assert!(
            received > 0,
            "expected non-empty frames from the loopback signal"
        );
        // Samples must stay in the transportable range.
        assert!(buf[..received * 2].iter().all(|x| x.abs() <= 1.0));
        // The fed tone should be audible, not silent.
        let peak = buf[..received * 2]
            .iter()
            .map(|x| x.abs())
            .fold(0.0f32, f32::max);
        assert!(
            peak > 1e-3,
            "loopback capture should carry the injected tone"
        );

        drop(player);
        let _ = loopback.kill();
        let _ = loopback.wait();
        let _ = std::fs::remove_file(&tone_path);
    }
}
