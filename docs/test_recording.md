# Manual hardware test — real Scarlett Solo takes

This is a manual (human-in-the-loop) verification of `carma record` against the
Focusrite Scarlett Solo USB interface on this machine. It is the acceptance
criterion for feature ticket 03 and should be run whenever the recorder's
capture path changes.

Performed by a human, not CI. CI uses the built-in fake source; this document
validates the real capture path, sample-accurate length, channel mapping, real
signal content and absence of dropouts/underruns on actual hardware.

## Prerequisites

- Scarlett Solo USB is plugged in, powered, and its input switches/Hi-Z are set
  sensibly (line/instrument levels loud enough to register but not clip).
- PipeWire is running (this is the default on this machine).
- carma is built. From the repo root:

  ```sh
  cargo build --release
  ```

  The binary is at `target/release/carma`.

## Step 0 — confirm the device is visible

```sh
carma devices
```

Find the Scarlett in the output. Record the friendly description exactly as
printed, e.g. `Scarlett Solo USB (Gen 3)` — you will pass a substring of it as
the `-d` selector. Confirm the two inputs appear (they show as one stereo
capture device). If the device is not listed, stop: the interface is not
exposed to PipeWire and nothing below will pass.

## Step 1 — sample-accurate length

Goal: a take of N measured seconds yields a WAV of ≈ N seconds (sample-accurate,
human-accepted).

1. Start a wall-clock timer for N = 5 seconds.
2. Record:

   ```sh
   target/release/carma record -d "Scarlett" -o step1_length.wav
   ```

3. At exactly the 5-second mark (timer), press Ctrl-C.
4. The CLI prints the written frame count. Check it against the timer:

   ```sh
   # expected ≈ 240000 frames (5 s × 48000 Hz)
   ffprobe -v error -show_entries format=duration -of default=noprint_wrappers=1:nokey=1 step1_length.wav
   ```

   Expected: duration ≈ 5.000 s and the CLI frame count ≈ 240000. Accept ± a
   couple of frames (a human pressing Ctrl-C late) — not a rounded or truncated
   duration. A duration clearly shorter than the timed take (e.g. 4.5 s) or one
   that snaps to and stays at a fixed maximum (suspect disk-full/stop failure)
   fails this step.

## Step 2 — both inputs captured as L/R

Goal: a distinct signal fed to each input is audible as that channel.

1. Feed a clearly distinguishable signal to the Scarlett's input 1 only
   (e.g. hum/talk into Mic 1, or a tone source). Leave input 2 silent.
   Record ~3 s:

   ```sh
   target/release/carma record -d "Scarlett" -o step2_lr.wav
   ```

2. Check each channel has real signal on input 1 and near-silence on input 2:

   ```sh
   ffmpeg -i step2_lr.wav -map_channel 0.0.0 -f null - 2>&1 | grep -E "max_volume|mean_volume"   # left (Mic 1)
   ffmpeg -i step2_lr.wav -map_channel 0.0.1 -f null - 2>&1 | grep -E "max_volume|mean_volume"   # right (Mic 2)
   ```

   Expected: `max_volume` on the fed channel is clearly above silence (> -30 dB,
   an actual human-readable level), and well above the unfed channel. Swap inputs
   (feed stream to input 2, silent input 1) and repeat; the louder channel must
   follow the fed input. If both channels are identical near-silent noise or both
   carry the same signal regardless of which input is fed, capture or L/R mapping
   is wrong and this fails.

## Step 3 — real signal content (not silence or noise)

Goal: playback is the actual captured signal, audible as such.

One of these, judged by ear:

- **Voice:** talk into the fed microphone for the whole take, then play the WAV.
  You must hear your words, not constant noise or a flat tone.
- **Tone:** feed a known musical tone/frequency, record, and play back. The pitch
  and rhythm must match what you played.

Compare the recording to the live signal as you feed it. If playback is
indistinguishable from un-gated noise, or the tonal content you fed is missing,
this fails.

## Step 4 — no underruns or dropouts

Goal: no reported underruns and no audible glitches over a longer take.

1. Record a ~30 s take with continuous signal (music/voice/tone):

   ```sh
   target/release/carma record -d "Scarlett" -o step4_long.wav
   ```

2. Listen to the entire file. There must be no clicks, skips, stutters or silent
   gaps mid-take. The CLI must not print any write-error / recording-failed
   message during or at the end.
3. Check the result is a complete, valid file:

   ```sh
   ffprobe -v error -show_entries stream=codec_name,sample_rate,channels -of default=noprint_wrappers=1 step4_long.wav
   ```

   Expected: `pcm_s24le`, 48000 Hz, 2 channels.

## Acceptance summary

Mark all passed:

- [ ] Device visible via `carma devices` (Scarlett listed as one stereo capture).
- [ ] N timed seconds → WAV ≈ N s, frame count ≈ N × 48000 (sample-accurate).
- [ ] Signal fed to input 1 is audible on left; fed to input 2 audible on right.
- [ ] Playback is the real captured signal (voice/tone recognisable).
- [ ] Long take has no audible dropouts and no reported underruns/errors.

All boxes checked = ticket 03 accepted.