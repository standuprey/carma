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
all the boxes in `test_recording.md`; no further code changes were requested.