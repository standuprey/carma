//! Audio capture sources behind the `interfaces::AudioSource` trait. The real
//! PipeWire backend lives here too; ticket 00 ships only the built-in fake so
//! tests and the CLI have a capture source without hardware.

#![forbid(unsafe_code)]

mod fake;

pub use fake::FakeAudioSource;