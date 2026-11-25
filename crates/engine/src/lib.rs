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
    AudioSource, Clock, Engine, EngineError, EngineStatus, Recorder, CHANNELS, QUANTUM,
};
use recorder::WavRecorder;

/// The concrete, wired engine.
pub struct CarmaEngine {
    source: Arc<Mutex<Box<dyn AudioSource + Send>>>,
    clock: Arc<Mutex<FrameClock>>,
    recorder: Arc<Mutex<Option<WavRecorder>>>,
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
        Self {
            source: Arc::new(Mutex::new(source)),
            clock: Arc::new(Mutex::new(FrameClock::default())),
            recorder: Arc::new(Mutex::new(None)),
            stop: Arc::new(AtomicBool::new(false)),
            frames: Arc::new(AtomicU64::new(0)),
            recording: Arc::new(AtomicBool::new(false)),
            handle: None,
        }
    }

    /// Records continuously until Ctrl-C, then finalizes. Installs the
    /// process-wide Ctrl-C handler and blocks until the signal fires.
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
        let _ = rx.recv();
        self.stop()
    }
}

impl Engine for CarmaEngine {
    fn start(&mut self, out_path: &Path) -> Result<(), EngineError> {
        if self.recording.load(Ordering::Acquire) {
            return Err(EngineError::Start("already recording".into()));
        }
        let recorder = WavRecorder::open(out_path, interfaces::SAMPLE_RATE, CHANNELS as u16)
            .map_err(|e| EngineError::Start(e.to_string()))?;
        *self.recorder.lock().unwrap() = Some(recorder);
        self.clock.lock().unwrap().reset();
        self.frames.store(0, Ordering::Relaxed);

        self.stop.store(false, Ordering::Release);
        self.recording.store(true, Ordering::Release);

        let source = Arc::clone(&self.source);
        let clock = Arc::clone(&self.clock);
        let recorder = Arc::clone(&self.recorder);
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
                frames.fetch_add(n, Ordering::Relaxed);
                let chunk = &buf[..n as usize * CHANNELS];
                match recorder.lock().unwrap().as_mut().map(|r| r.push(chunk)) {
                    Some(Ok(())) => {}
                    Some(Err(e)) => {
                        // Stop capturing on a write error; finalize still runs
                        // in `stop` so the file is left valid. Disk-full
                        // handling is refined in the robustness ticket.
                        eprintln!("recording error: {e}");
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
            if let Err(e) = r.finalize() {
                problems.push(format!("finalize failed: {e}"));
            }
        }

        self.recording.store(false, Ordering::Release);
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
    use recorder::read_wav_header;

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
}
