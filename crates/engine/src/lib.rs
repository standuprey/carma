//! Composition root: wires the fake source, the frame clock and the WAV
//! recorder behind the `interfaces::Engine` facade. This is the only crate that
//! knows how the modules fit together; nothing outside it reaches into the
//! concrete clock/audio-io/recorder types. It also owns signal handling: the
//! Ctrl-C handler that ends a recording lives here.

#![forbid(unsafe_code)]

use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};

use audio_io::FakeAudioSource;
use clock::FrameClock;
use interfaces::{
    AudioSource, Clock, Engine, EngineError, EngineStatus, Recorder, RecorderError, CHANNELS,
    QUANTUM,
};
use recorder::WavRecorder;

/// Builds the recorder backing one take (defaults to a real WAV file);
/// injectable so tests can substitute failing recorders.
type RecorderFactory = Box<dyn Fn(&Path) -> Result<Box<dyn Recorder + Send>, RecorderError> + Send>;

/// The concrete, wired engine.
pub struct CarmaEngine {
    source: Arc<Mutex<Box<dyn AudioSource + Send>>>,
    clock: Arc<Mutex<FrameClock>>,
    recorder: Arc<Mutex<Option<Box<dyn Recorder + Send>>>>,
    recorder_factory: RecorderFactory,
    /// Set by the record worker when a push fails; surfaced by [`Engine::stop`].
    write_error: Arc<Mutex<Option<RecorderError>>>,
    stop: Arc<AtomicBool>,
    frames: Arc<AtomicU64>,
    recording: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl Default for CarmaEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl CarmaEngine {
    pub fn new() -> Self {
        Self::with_source(Box::new(FakeAudioSource::new()))
    }

    /// Builds an engine whose capture source is the given PipeWire device.
    pub fn for_device(device: audio_io::Device) -> Result<Self, EngineError> {
        let source = audio_io::PipeWireAudioSource::new(&device)
            .map_err(|e| EngineError::Start(format!("pipewire: {e}")))?;
        Ok(Self::with_source(Box::new(source)))
    }

    /// Lists the capture devices visible on the PipeWire graph.
    pub fn list_devices() -> Result<Vec<audio_io::Device>, EngineError> {
        audio_io::list_capture_devices().map_err(|e| EngineError::Start(format!("pipewire: {e}")))
    }

    /// Resolves a user-supplied device selector to a capture device.
    pub fn find_capture_device(selector: &str) -> Result<Option<audio_io::Device>, EngineError> {
        audio_io::find_capture_device(selector)
            .map_err(|e| EngineError::Start(format!("pipewire: {e}")))
    }

    fn with_source(source: Box<dyn AudioSource + Send>) -> Self {
        Self::with_parts(
            source,
            Box::new(|path| {
                let r = WavRecorder::open(path, interfaces::SAMPLE_RATE, CHANNELS as u16)?;
                Ok(Box::new(r) as Box<dyn Recorder + Send>)
            }),
        )
    }

    /// Builds an engine that records through the given factory instead of the
    /// default WAV file recorder.
    pub fn with_recorder_factory(factory: RecorderFactory) -> Self {
        Self::with_parts(Box::new(FakeAudioSource::new()), factory)
    }

    fn with_parts(source: Box<dyn AudioSource + Send>, recorder_factory: RecorderFactory) -> Self {
        Self {
            source: Arc::new(Mutex::new(source)),
            clock: Arc::new(Mutex::new(FrameClock::default())),
            recorder: Arc::new(Mutex::new(None)),
            recorder_factory,
            write_error: Arc::new(Mutex::new(None)),
            stop: Arc::new(AtomicBool::new(false)),
            frames: Arc::new(AtomicU64::new(0)),
            recording: Arc::new(AtomicBool::new(false)),
            handle: None,
        }
    }

    /// Records continuously until Ctrl-C or until the take ends on a write
    /// error (e.g. disk full), then finalizes and surfaces the outcome.
    /// Installs the process-wide Ctrl-C handler and blocks until either
    /// happens.
    pub fn record_until_signal(&mut self, out_path: &Path) -> Result<(), EngineError> {
        self.start(out_path)?;
        let (tx, rx) = mpsc::channel::<()>();
        let signaler = tx.clone();
        ctrlc::set_handler(move || {
            let _ = signaler.send(());
        })
        .map_err(|e| {
            // Recording already began; stop cleanly so the worker exits and the
            // WAV is finalized to a valid file before we report the error.
            let _ = self.stop();
            EngineError::Start(format!("cannot install signal handler: {e}"))
        })?;
        // Keep the sender alive so `rx` does not close while recording.
        let _alive = tx;
        // Wait for Ctrl-C, but also end promptly when the record worker stops
        // on a write error: the take is over and the caller must learn it
        // failed instead of hanging until another signal.
        self.wait_for_stop(&rx);
        self.stop()
    }

    /// Blocks until the take ends: Ctrl-C (a message on `rx`), a write error
    /// in the record worker, or an external stop.
    fn wait_for_stop(&self, rx: &mpsc::Receiver<()>) {
        loop {
            if rx
                .recv_timeout(std::time::Duration::from_millis(50))
                .is_ok()
            {
                return;
            }
            if self.write_error.lock().unwrap().is_some() || !self.recording.load(Ordering::Acquire)
            {
                return;
            }
        }
    }
}

impl Engine for CarmaEngine {
    fn start(&mut self, out_path: &Path) -> Result<(), EngineError> {
        if self.recording.load(Ordering::Acquire) {
            return Err(EngineError::Start("already recording".into()));
        }
        let recorder =
            (self.recorder_factory)(out_path).map_err(|e| EngineError::Start(e.to_string()))?;
        *self.recorder.lock().unwrap() = Some(recorder);
        self.write_error.lock().unwrap().take();
        self.clock.lock().unwrap().reset();
        self.frames.store(0, Ordering::Relaxed);

        self.stop.store(false, Ordering::Release);
        self.recording.store(true, Ordering::Release);

        let source = Arc::clone(&self.source);
        let clock = Arc::clone(&self.clock);
        let recorder = Arc::clone(&self.recorder);
        let write_error = Arc::clone(&self.write_error);
        let stop = Arc::clone(&self.stop);
        let frames = Arc::clone(&self.frames);

        let handle = std::thread::spawn(move || {
            let mut buf = vec![0.0f32; QUANTUM as usize * CHANNELS];
            loop {
                if stop.load(Ordering::Acquire) {
                    break;
                }
                let n = source.lock().unwrap().read(&mut buf);
                if n == 0 {
                    // Paced source: no frames due yet; retry shortly.
                    continue;
                }
                let n = n as u64;
                clock.lock().unwrap().advance(n);
                let chunk = &buf[..n as usize * CHANNELS];
                match recorder.lock().unwrap().as_mut().map(|r| r.push(chunk)) {
                    Some(Ok(())) => {
                        frames.fetch_add(n, Ordering::Relaxed);
                    }
                    Some(Err(e)) => {
                        // Stop capturing on a write error and remember it so
                        // `stop` can surface it after finalizing the take.
                        *write_error.lock().unwrap() = Some(e);
                        break;
                    }
                    None => break,
                }
            }
        });
        self.handle = Some(handle);
        Ok(())
    }

    fn stop(&mut self) -> Result<(), EngineError> {
        if !self.recording.load(Ordering::Acquire) {
            return Ok(());
        }
        self.stop.store(true, Ordering::Release);

        // Join the worker regardless of outcome so finalize always runs below.
        // A panicked worker must not skip WAV finalization.
        let mut problems = Vec::new();
        if let Some(handle) = self.handle.take() {
            if handle.join().is_err() {
                problems.push("record thread panicked".to_string());
            }
        }

        // Recover a poisoned lock: if the worker panicked while holding it, we
        // still take and finalize the recorder so the file is left valid.
        let recorder = self
            .recorder
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        if let Some(mut r) = recorder {
            self.frames.store(r.frames_written(), Ordering::Relaxed);
            if let Err(e) = r.finalize() {
                problems.push(format!("finalize failed: {e}"));
            }
        }

        self.recording.store(false, Ordering::Release);

        // Surface a take that ended on a write error: the partial file is
        // finalized above, but the caller must learn the take did not complete.
        if let Some(err) = self.write_error.lock().unwrap().take() {
            let cause = match err {
                RecorderError::DiskFull(_) => "disk full",
                _ => "write error",
            };
            let frames_written = self.frames.load(Ordering::Relaxed);
            problems.push(format!(
                "{cause}: recording stopped, wrote {frames_written} frames"
            ));
        }

        if problems.is_empty() {
            Ok(())
        } else {
            Err(EngineError::Stop(problems.join("; ")))
        }
    }

    fn status(&self) -> EngineStatus {
        EngineStatus {
            recording: self.recording.load(Ordering::Acquire),
            written_frames: self.frames.load(Ordering::Relaxed),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use recorder::{read_wav_header, testing::FailAfterWriter};

    #[test]
    fn records_a_valid_stereo_48k_wav_matching_duration() {
        let path = std::env::temp_dir().join(format!("carma_e2e_{}.wav", std::process::id()));
        let mut engine = CarmaEngine::new();
        engine.start(&path).unwrap();

        let done = {
            let mut done = 0u64;
            let deadline = std::time::Instant::now() + std::time::Duration::from_millis(400);
            while std::time::Instant::now() < deadline {
                let s = engine.status();
                if s.written_frames > 18_000 {
                    // ~0.4 s at 48 kHz captured; enough to assert duration.
                    done = s.written_frames;
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            done
        };

        // The paced fake source must have streamed real-time frames.
        assert!(
            done > 1000,
            "engine should have captured frames, got {done}"
        );
        engine.stop().unwrap();
        let status = engine.status();
        assert!(!status.recording, "engine must report stopped");

        let h = read_wav_header(&path).unwrap();
        assert_eq!(h.format, 1, "PCM format tag");
        assert_eq!(h.bits_per_sample, 24, "bits per sample");
        assert_eq!(h.sample_rate, 48_000, "sample rate");
        assert_eq!(h.channels, 2, "channels");
        assert_eq!(
            u64::from(h.num_frames),
            status.written_frames,
            "header frame count must match recorded duration"
        );

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn disk_full_yields_partial_wav_and_surfaces_error() {
        let path = std::env::temp_dir().join(format!("carma_diskfull_{}.wav", std::process::id()));
        // Room for 300 stereo frames (300 * 2 channels * 3 bytes) of body.
        let budget = 300 * 2 * 3;
        let mut engine = CarmaEngine::with_recorder_factory(Box::new(move |path| {
            let file = std::fs::File::create(path).map_err(|e| RecorderError::Io(e.to_string()))?;
            let writer = FailAfterWriter::new(
                file,
                budget,
                std::io::Error::new(std::io::ErrorKind::StorageFull, "no space left on device"),
            );
            let r = WavRecorder::from_writer(writer, interfaces::SAMPLE_RATE, CHANNELS as u16)?;
            Ok(Box::new(r) as Box<dyn Recorder + Send>)
        }));

        engine.start(&path).unwrap();
        // The fake source paces at 48 kHz; the failing write happens on the
        // first block (~21 ms), well within this window.
        std::thread::sleep(std::time::Duration::from_millis(300));

        let stop = engine.stop();
        let message = stop.unwrap_err().to_string();
        assert!(
            message.contains("disk full"),
            "must classify the failure as disk full, got: {message}"
        );
        assert!(
            message.contains("stopped, wrote 300 frames"),
            "must report the frames actually written, got: {message}"
        );
        assert!(!engine.status().recording, "engine must report stopped");

        let h = read_wav_header(&path).unwrap();
        assert_eq!(
            h.num_frames, 300,
            "partial header must match written frames"
        );
        assert_eq!(
            engine.status().written_frames,
            300,
            "surfaced frame count must match the recorded bytes"
        );

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn take_ending_on_disk_full_stops_without_a_signal() {
        let path = std::env::temp_dir().join(format!("carma_autostop_{}.wav", std::process::id()));
        let budget = 300 * 2 * 3;
        let mut engine = CarmaEngine::with_recorder_factory(Box::new(move |path| {
            let file = std::fs::File::create(path).map_err(|e| RecorderError::Io(e.to_string()))?;
            let writer = FailAfterWriter::new(
                file,
                budget,
                std::io::Error::new(std::io::ErrorKind::StorageFull, "no space left on device"),
            );
            let r = WavRecorder::from_writer(writer, interfaces::SAMPLE_RATE, CHANNELS as u16)?;
            Ok(Box::new(r) as Box<dyn Recorder + Send>)
        }));
        engine.start(&path).unwrap();

        // No signal is sent: `wait_for_stop` must end on its own when the
        // disk-full write fails, and `stop` must surface the failure.
        let (_tx, rx) = mpsc::channel::<()>();
        engine.wait_for_stop(&rx);
        let message = engine.stop().unwrap_err().to_string();
        assert!(
            message.contains("disk full"),
            "must classify the failure as disk full, got: {message}"
        );
        assert!(
            message.contains("stopped, wrote 300 frames"),
            "must report the frames actually written, got: {message}"
        );

        let h = read_wav_header(&path).unwrap();
        assert_eq!(
            h.num_frames, 300,
            "partial header must match written frames"
        );

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn sigint_finalizes_valid_sample_accurate_wav() {
        use std::process::Command;
        use std::time::{Duration, Instant};

        let path = std::env::temp_dir().join(format!("carma_sigint_{}.wav", std::process::id()));
        let wav_path = path.clone();
        let mut engine = CarmaEngine::new();
        let started = Instant::now();
        let worker = std::thread::spawn(move || engine.record_until_signal(&wav_path));

        std::thread::sleep(Duration::from_millis(500));
        let pid = std::process::id();
        let kill = Command::new("kill")
            .args(["-INT", &pid.to_string()])
            .status()
            .expect("kill must run");
        assert!(kill.success(), "SIGINT must be deliverable");

        let result = worker.join().expect("record thread must not panic");
        assert!(result.is_ok(), "clean Ctrl-C stop must succeed: {result:?}");
        let elapsed_secs = started.elapsed().as_secs_f64();

        let h = read_wav_header(&path).unwrap();
        let expected = elapsed_secs * 48_000.0;
        let delta = h.num_frames as f64 - expected;
        assert!(
            delta.abs() < 48_000.0 * 0.75,
            "length must be sample-accurate (frame count {}) vs elapsed {:.2}s",
            h.num_frames,
            elapsed_secs
        );
        assert!(h.num_frames > 0, "take must not be empty");

        std::fs::remove_file(&path).ok();
    }
}
