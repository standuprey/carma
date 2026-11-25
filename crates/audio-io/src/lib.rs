//! Audio capture sources behind the `interfaces::AudioSource` trait. The real
//! PipeWire backend lives here too; ticket 00 ships only the built-in fake so
//! tests and the CLI have a capture source without hardware.

#![forbid(unsafe_code)]

mod fake;
pub mod pipewire;

pub use fake::FakeAudioSource;
pub use pipewire::{
    find_capture_device, find_node, list_capture_devices, AudioIoError, Device, PipeWireAudioSource,
};
