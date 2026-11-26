//! A paced, deterministic capture source: a 1 kHz sine, stereo, float32.

use std::time::{Duration, Instant};

use interfaces::{AudioSource, CHANNELS, SAMPLE_RATE};

const HZ: f64 = 1000.0;

/// The fake capture port. It paces its output to wall clock so the pipeline
/// observes realistic timing (≈`SAMPLE_RATE` frames/second) instead of
/// draining as fast as the CPU allows, which is what makes the recorded
/// duration sample-accurate.
#[derive(Debug)]
pub struct FakeAudioSource {
    channels: usize,
    phase: f64,
    started: Instant,
    emitted: u64,
}

impl Default for FakeAudioSource {
    fn default() -> Self {
        Self::new()
    }
}

impl FakeAudioSource {
    pub fn new() -> Self {
        Self {
            channels: CHANNELS,
            phase: 0.0,
            started: Instant::now(),
            emitted: 0,
        }
    }
}

impl AudioSource for FakeAudioSource {
    fn sample_rate(&self) -> u32 {
        SAMPLE_RATE
    }

    fn channels(&self) -> usize {
        self.channels
    }

    fn read(&mut self, out: &mut [f32]) -> usize {
        let max = out.len() / self.channels;
        let elapsed = self.started.elapsed();
        let target = (elapsed.as_secs_f64() * SAMPLE_RATE as f64) as u64;
        let available = (target as i64 - self.emitted as i64).clamp(0, max as i64) as usize;
        if available == 0 {
            // No frames due against the wall clock yet; pause to avoid a busy
            // loop and let the caller retry.
            std::thread::sleep(Duration::from_millis(1));
            return 0;
        }
        let omega = 2.0 * std::f64::consts::PI * HZ / SAMPLE_RATE as f64;
        for i in 0..available {
            let s = (self.phase).sin() as f32;
            out[i * self.channels] = s;
            out[i * self.channels + 1] = s;
            self.phase += omega;
            self.emitted += 1;
        }
        available
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_stereo_48k() {
        let s = FakeAudioSource::new();
        assert_eq!(s.sample_rate(), 48_000);
        assert_eq!(s.channels(), 2);
    }

    #[test]
    fn emits_interleaved_scaled_wave_after_pacing() {
        let mut s = FakeAudioSource::new();
        // Let real time catch up so a block is due.
        std::thread::sleep(Duration::from_millis(40));
        let mut buf = [0.0f32; 7 * 2];
        let written = s.read(&mut buf);
        assert!(written > 0);
        assert!(written <= 7);
        let (head, _rest) = buf.split_at(written * s.channels());
        assert!(!head.iter().all(|x| *x == 0.0), "source must not be silent");
        assert!(
            head.iter().all(|x| x.abs() <= 1.0),
            "samples must be within [-1.0, 1.0]"
        );
        // A mono sine is duplicated across the stereo pair.
        for i in 0..written {
            assert_eq!(head[i * 2], head[i * 2 + 1]);
        }
    }
}
