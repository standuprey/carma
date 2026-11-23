//! Declares the module traits only: the exchangeable boundaries between carma
//! modules, plus the shared streaming constants. Contains no logic — each
//! implementation lives in its own crate and is wired in `engine`.

#![forbid(unsafe_code)]

use std::path::Path;

/// Fixed in-process sample rate (Hz).
pub const SAMPLE_RATE: u32 = 48_000;
/// Fixed streaming quantum (frames per audio block).
pub const QUANTUM: u32 = 1024;
/// Stereo channel count transported by every in-process source.
pub const CHANNELS: usize = 2;

/// A streaming audio capture source delivering interleaved blocks of
/// `channels()` f32 samples per frame, each in `[-1.0, 1.0]`.
pub trait AudioSource {
    fn sample_rate(&self) -> u32;
    fn channels(&self) -> usize;
    /// Copies up to `out.len() / self.channels()` frames, interleaved, into
    /// `out`. Returns the number of frames written, which may be fewer than
    /// requested (the source is paced in real time). A return of `0` means no
    /// frames are ready yet, not end-of-stream.
    fn read(&mut self, out: &mut [f32]) -> usize;
}

/// A passive, sample-accurate timeline: a frame counter.
pub trait Clock {
    fn now(&self) -> u64;
    fn advance(&mut self, frames: u64);
    fn reset(&mut self);
}

/// Consumes an interleaved stereo f32 frame stream and persists it. Writing is
/// append-only until [`Recorder::finalize`] closes the stream and fixes its
/// header; [`Recorder::finalize`] must leave a valid file even on failure.
pub trait Recorder {
    fn push(&mut self, buf: &[f32]) -> Result<(), RecorderError>;
    fn finalize(&mut self) -> Result<(), RecorderError>;
}

/// Run state surfaced through the engine control facade.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EngineStatus {
    pub recording: bool,
    pub written_frames: u64,
}

/// The engine control facade. The only place external callers (the CLI) reach
/// the modules; the concrete wiring lives behind this trait.
pub trait Engine {
    /// Starts capturing into `out_path`. Recording runs asynchronously.
    fn start(&mut self, out_path: &Path) -> Result<(), EngineError>;
    /// Stops capture, joins the record thread and finalizes the output.
    fn stop(&mut self) -> Result<(), EngineError>;
    fn status(&self) -> EngineStatus;
}

#[derive(Debug, thiserror::Error)]
pub enum RecorderError {
    #[error("recorder io error: {0}")]
    Io(String),
    #[error("recorder already finalized")]
    AlreadyFinalized,
    #[error("misaligned frame buffer")]
    Misaligned,
}

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("engine start failed: {0}")]
    Start(String),
    #[error("engine stop failed: {0}")]
    Stop(String),
}