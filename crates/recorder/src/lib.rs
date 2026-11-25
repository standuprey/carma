//! Consumes an interleaved stereo f32 frame stream and writes it to a
//! 24-bit PCM WAV/RIFF file. `finalize` fixes the header so the output is
//! always parseable.

#![forbid(unsafe_code)]

mod wav;

pub use wav::{read_wav_header, WavHeader, WavRecorder};