---
id: "00"
title: Walking skeleton: interfaces crate wired end-to-end with a fake capture source
kind: HITL
blocked_by: []
touches: [interfaces, clock, audio-io, recorder, engine, cli]
verify: cli
unsafe_with: []
---

## Outcome
The whole pipeline exists as real, compiling, wired code: `carma record -o
out.wav` records from the built-in fake source and produces a valid WAV. The
module interfaces are reviewed here as code, not on paper.

## Scope
- A Cargo workspace with one crate per module (`interfaces`, `clock`,
  `audio-io`, `recorder`, `engine`, `cli`).
- The `interfaces` crate: traits for `AudioSource`, `Clock`, `Recorder`, and the
  engine control facade.
- `clock`: a sample-accurate frame counter timeline.
- `audio-io`: `FakeAudioSource` (stereo f32 @ 48 kHz, e.g. a 1 kHz sine).
- `recorder`: consumes stereo f32 frames, writes 24-bit PCM WAV, finalizes on stop.
- `engine`: composition root that wires the modules and exposes start/stop/status.
- `cli`: `carma record -o out.wav`, driven through the engine facade.
- A `cargo test` that records from the fake source and asserts the WAV header.

## Example
```text
# pseudocode
cargo build --workspace
cargo run -p cli -- record -o out.wav      # records from FakeAudioSource until Ctrl-C
-> out.wav: WAV/RIFF, 24-bit PCM, 48000 Hz, stereo
```

## Out of scope
Real PipeWire capture, `carma devices`, disk-full handling, real Scarlett hardware.

## Done when
- The workspace compiles and `cargo test` passes.
- Recording from the fake source yields a WAV whose header asserts: PCM format
  tag, 24 bits/sample, 48000 Hz, 2 channels, and a frame count matching the
  recorded duration.
- A human reviews the interfaces in this PR: the traits are minimal, and the CLI
  reaches modules only through the engine control facade.