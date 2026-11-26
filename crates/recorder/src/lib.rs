//! Consumes an interleaved stereo f32 frame stream and writes it to a
//! 24-bit PCM WAV/RIFF file. `finalize` fixes the header so the output is
//! always parseable, even when a take fails mid-write.

#![forbid(unsafe_code)]

pub mod testing;
mod wav;

pub use wav::{read_wav_header, WavHeader, WavRecorder};
