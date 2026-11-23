//! Sample-accurate timeline: a passive frame counter. Transport/tempo lives
//! behind the same `Clock` interface later; for now the counter is the only
//! position source.

#![forbid(unsafe_code)]

use interfaces::Clock;

/// A monotonic, wrap-free frame counter. Negative positions are impossible;
/// `advance` only ever moves forward.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct FrameClock {
    frames: u64,
}

impl Clock for FrameClock {
    fn now(&self) -> u64 {
        self.frames
    }

    fn advance(&mut self, frames: u64) {
        self.frames = self.frames.wrapping_add(frames);
    }

    fn reset(&mut self) {
        self.frames = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_at_zero_and_clamps_reset() {
        let mut c = FrameClock::default();
        assert_eq!(c.now(), 0);
        c.advance(1024);
        c.advance(512);
        assert_eq!(c.now(), 1536);
        c.reset();
        assert_eq!(c.now(), 0);
    }
}