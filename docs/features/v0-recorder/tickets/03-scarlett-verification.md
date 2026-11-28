---
id: "03"
title: Human verification of real Scarlett Solo takes
kind: HITL
blocked_by: ["01"]
touches: [audio-io, recorder]
verify: cli
unsafe_with: []
---

## Outcome
A human performs and accepts real recordings on the Scarlett Solo USB,
confirming sample-accurate length, capture of both inputs as L/R, real signal
content, and no audible glitches.

## Scope
- Run `carma record` on the Scarlett and capture both mic inputs as stereo
  (L = Mic1, R = Mic2).
- Verify recorded length is sample-accurate against a timed take.
- Verify playback is the actual microphone signal (not silence or noise) on both
  channels.
- Confirm no reported underruns or audible dropouts.

## Example
```text
# pseudocode
record for a measured 5 seconds -> WAV duration ≈ 5.000 s
feed a distinct signal to each of the two inputs
-> left and right channels contain their respective signals
```

## Out of scope
Any feature changes; this ticket is verification on real hardware only.

## Done when
- N timed seconds of recording produce a WAV of ≈ N seconds (sample-accurate),
  accepted by a human.
- A distinct signal fed to each of the two inputs is audible as left and right
  respectively.
- Playback is the real captured signal with no audible dropouts and no reported
  underruns.

## Requirement confirmation
Confirmed 2026-11-28. The deliverable for this ticket is the step-by-step human
test procedure at `docs/test_recording.md` (scope: a real Scarlett Solo take
verifying sample-accurate length, L/R input mapping, real signal content, and no
underruns/dropouts). Acceptance is the human running that procedure and checking
all the boxes in `test_recording.md`.

**Updated 2026-11-28 (user-directed):** while running the procedure, the human
step surfaced a real-capture bug: `carma record` on a real ALSA node writes ~0
frames (`pw-cat` records the same node fine). The user approved folding the fix
into this ticket. Scope becomes: make real capture stream correctly (the
audio-io bug), then the human re-runs `test_recording.md` for acceptance.

**Diagnosis and fix (2026-11-28):** root cause is in `audio-io`'s `read()`
(`PipeWireAudioSource`). When the FIFO was empty it slept `1 ms` **while holding
the FIFO mutex**. The PipeWire process callback (real-time thread) must lock that
same mutex to push new samples, so the held-while-sleeping lock repeatedly
starved the callback: the stream ran for only a few blocks or zero, with no
sustained real capture. The fix is to drop the mutex guard before the
empty-check sleep, so the real-time producer is never blocked. This was isolated
by throwaway harnesses: with the real-code shape minus the sleep-holding-lock,
real capture streamed ~115 process callbacks (≈230k samples) in 2.5 s on a real
Scarlett Mic1 node; the full CLI is fixed in `crates/audio-io/src/pipewire.rs`
(`read()` drops the lock before sleeping). After the fix the CLI records full
real takes (~143k frames / 3 s) on the Scarlett Mic1/Mic2, webcam and internal
mic, and the previously-flaky loopback test passes reliably. `cargo test` is
green and fmt/clippy clean on `audio-io`.