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

## Requirement confirmation

Settled with the user before implementation (2026-11-26):
- A take that ends on a write error/disk-full exits the CLI with status 1
  (failure) and reports the cause on stderr, e.g.
  `recording failed: disk full: recording stopped, wrote N frames`. A clean
  Ctrl-C stop stays exit 0.
- A documented, test-only env var `CARMA_CAPTURE=fake` makes `carma record`
  use the built-in fake source instead of a PipeWire device, so interruption
  is testable at the command boundary without hardware or a PipeWire daemon.
- The recorder drops its intermediate `BufWriter`/buffered accounting so bytes
  written are tracked exactly at the point `write_all` succeeds; the header size
  fields are patched even when a preceding flush failed, so a partial file is
  self-consistent (`finalize` reports the underlying error but leaves a valid
  header whenever the small in-place patch can land — mirroring real filesystem
  behavior where preallocated header blocks stay writable on ENOSPC).

## Done when
- A simulated write error/disk-full (test double) leaves a WAV whose header
  matches the bytes actually written and surfaces an error without a panic.
- SIGINT during record finalizes a valid WAV with a correct header and a
  sample-accurate length.
- The CLI surfaces a failed take (write error/disk-full) with exit status 1 and
  the frame count actually written; a clean Ctrl-C stop stays exit 0.
- `cargo test` passes; no test leaves a file with a truncated or mismatched
  header.