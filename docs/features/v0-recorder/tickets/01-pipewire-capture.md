---
id: "01"
title: PipeWire capture backend and device enumeration
kind: AFK
blocked_by: ["00"]
touches: [audio-io, cli]
verify: cli
unsafe_with: []
---

## Outcome
`audio-io` gains a real PipeWire capture backend: `carma devices` lists the
Scarlett's inputs and `carma record -d <device>` captures real frames from the
chosen source into the recorder.

## Scope
- A real `AudioSource` over libpipewire (Rust `pipewire` crate), to capture the
  Scarlett's two mic inputs as a stereo f32 stream (L = Mic1, R = Mic2).
- Enumerate capture sources from the PipeWire registry.
- `carma devices` prints available sources.
- The engine wires the real backend for `record -d <device>`; `FakeAudioSource`
  remains for tests.

## Example
```text
# pseudocode
cargo run -p cli -- devices
-> Scarlett Solo USB: Mic1, Mic2
cargo run -p cli -- record -d "Scarlett Solo USB" -o take.wav
-> take.wav holds real frames; Mic1 on left, Mic2 on right
```

## Out of scope
Playback and live monitoring (the Scarlett's hardware monitoring handles that).

## Done when
- `cargo test` passes, including a test that connects a PipeWire context and
  receives at least one non-empty frame block from a loopback signal (no human
  speech required).
- `carma devices` shows the Scarlett's inputs.
- Recording via PipeWire produces a WAV with a valid header and non-silent
  content when a known signal is fed to an input.