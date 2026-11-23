---
id: "02"
title: Clean stop and disk-full handling in the recorder
kind: AFK
blocked_by: ["00"]
touches: [recorder, engine, cli]
verify: cli
unsafe_with: []
---

## Outcome
Recording stops cleanly on Ctrl-C (SIGINT), finalizing a valid WAV with a
sample-accurate length; a disk-full or write error never crashes and yields a
valid partial file, not a corrupt one.

## Scope
- The recorder's write path handles errors/disk-full: finalize the header with
  the frames actually written, surface the error through the engine, and leave
  no truncated or mismatched header.
- SIGINT (Ctrl-C) handling in the engine routes to a controlled stop that
  finalizes the recorder.
- The engine surfaces recording status/errors to the CLI.

## Example
```text
# pseudocode
disk fills mid-take
-> recording stops; partial WAV has a correct header and the frames actually
   written; cli reports "disk full: recording stopped, wrote N frames"
Ctrl-C mid-record
-> out.wav is valid and its duration ≈ elapsed time
```

## Out of scope
Retry logic, spooling, or output formats other than WAV.

## Done when
- A simulated write error/disk-full (test double) leaves a WAV whose header
  matches the bytes actually written and surfaces an error without a panic.
- SIGINT during record finalizes a valid WAV with a correct header and a
  sample-accurate length.
- `cargo test` passes; no test leaves a file with a truncated or mismatched
  header.