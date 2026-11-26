# carma — v0 CLI recorder

## 1. Outcome

carma is a modular Linux audio/music application. Its first deliverable is a
command-line recorder: `carma record` captures stereo audio from any available
audio input (e.g. the Focusrite Scarlett Solo USB interface) to a 24-bit,
48 kHz WAV file with sample-accurate length, and stops cleanly. The codebase establishes the modular
architecture the rest of carma builds on — each module behind a trait, wired in
one place.

## 2. Goals / non-goals

Goals
- `carma devices` lists available capture sources (the Scarlett's two mic inputs).
- `carma record -d <device> [-o out.wav]` records stereo from a real capture
  device; Ctrl-C stops and finalizes a valid WAV.
- Modules are exchangeable: every module implements a trait declared in the
  `interfaces` crate and is wired only in `engine`.
- Recording length is sample-accurate.

Non-goals (v0)
- No playback engine, overdub, multitrack, TUI/GUI, MIDI, DSP/effects, plugin
  hosting, tempo/beat transport, non-WAV output, config files, or IPC.
- Nothing user-tunable: sample rate fixed at 48000 Hz, quantum fixed at 1024.

## 3. Decisions

- **Language Rust**, Cargo workspace, one crate per module, `interfaces` crate
  holding only traits. Reason: compiled and GC-free (fits low-latency audio),
  compile-time-concurrency-safe, and the workspace makes module boundaries
  compiler-enforced rather than convention.
- **PipeWire backend first** behind the `audio-io` interface; the system already
  runs PipeWire (handles hotplug and format negotiation). An ALSA backend can be
  added later behind the same interface.
- **clock is the core module**: a passive, sample-accurate timeline (frame
  counter). Transport/tempo is reserved behind the same interface for later.
- **Single process**, multi-threaded. Streaming (audio blocks) travels over
  lock-free SPSC ring buffers; control uses direct API calls plus an async event
  channel. No IPC in v0.
- **The CLI is abstracted from the modules**: it calls the `engine` control
  facade only and never reaches into `clock`, `audio-io`, or `recorder`.
- **Streaming parameters**: 1024-sample quantum @ 48000 Hz, ring buffer ≈ 10
  periods, float32 in-process (PipeWire converts the device's s32le to f32).
- **Output**: WAV/RIFF, 24-bit PCM, 48000 Hz, stereo (both Scarlett inputs
  feeding L/R).
- **Verification device**: the Scarlett Solo USB (present on the machine).
  CI-able verification uses a built-in `FakeAudioSource`; real-hardware
  verification is a named human step.

## 4. Module map

| Module / touches key | Code or artifact paths | Responsibility | Depends on | New or existing |
|---|---|---|---|---|
| `interfaces` | `crates/interfaces` | Declares the traits only: `AudioSource`, `Clock`, `Recorder`, and the engine control facade. No logic. | none | new |
| `clock` | `crates/clock` | Passive sample timeline: frame counter and sample-accurate position. | `interfaces` | new |
| `audio-io` | `crates/audio-io` | PipeWire capture backend plus the built-in `FakeAudioSource`; device/source enumeration; emits a stereo f32 frame stream. | `interfaces` | new |
| `recorder` | `crates/recorder` | Consumes the frame stream, writes 24-bit PCM WAV, finalizes a valid header on stop; handles write errors/disk-full. | `interfaces` | new |
| `engine` | `crates/engine` | Composition root and control facade (start/stop/status/device); the only place modules are wired; owns signal handling. | `interfaces`, `clock`, `audio-io`, `recorder` | new |
| `cli` | `crates/cli` | Parses `devices` / `record`, prints status; calls the engine control facade only. | `interfaces` | new |

## 5. Scale and risks

Scale does not matter for v0: single device, single process, bounded frame
format (48 kHz × 2 × 24-bit ≈ 288 KB/s ≈ ~1 GB/hour). The one unbounded-growth
risk is the recording file filling the disk, so the recorder must handle write
errors/disk-full by finalizing a valid partial WAV and surfacing an error —
never crashing and never returning a corrupt file.

Reversibility: greenfield — no deployed device, migration, protocol, or config
to migrate. The only semi-permanent artifact is the `interfaces` crate; it is
reviewed as real compiling code in ticket 00's PR rather than on paper.

## 6. Verification

- `cargo test` passes: tests at the useful boundaries (clock frame counting,
  recorder WAV header format/rate/channels/frame count, engine start/stop
  wiring, ring overrun handling).
- Automated: record from the built-in `FakeAudioSource` → valid WAV with a
  correct header.
- Human: real takes on the Scarlett Solo USB. Criteria: recording for a measured
  N seconds yields a WAV of ≈ N seconds (sample-accurate); both inputs are
  captured on L/R resp.; playback is the actual microphone signal; no reported
  underruns. A human accepts in ticket 03.

## 7. Follow-ups

- Tempo/beat transport behind the `Clock` interface.
- Playback engine.
- TUI/GUI.
- MIDI input.
- Plugin hosting — re-evaluate the C++ ecosystem if this develops into a DAW.