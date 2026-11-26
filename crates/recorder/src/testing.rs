//! Doubles for the recorder's write port, so error and disk-full behavior can
//! be exercised deterministically without filling a real disk. They live next
//! to the port they simulate (the `WavRecorder` write target).

use std::fs::File;
use std::io::{self, Seek, SeekFrom, Write};

/// Byte offset where a WAV body begins (44-byte canonical header).
const BODY_OFFSET: u64 = 44;

/// A write target that accepts body bytes only up to a fixed budget, then
/// fails every further body write. Header-region writes (offsets < 44) always
/// succeed, mirroring real filesystems where a full disk still allows
/// in-place overwrite of already-allocated header blocks.
#[derive(Debug)]
pub struct FailAfterWriter {
    inner: File,
    pos: u64,
    /// Absolute file offset above which body writes are refused.
    body_region_limit: u64,
    err: io::Error,
}

impl FailAfterWriter {
    /// `body_bytes_budget` is the number of payload bytes accepted after the
    /// 44-byte header before writes start failing.
    pub fn new(inner: File, body_bytes_budget: u64, err: io::Error) -> Self {
        Self {
            inner,
            pos: 0,
            body_region_limit: BODY_OFFSET + body_bytes_budget,
            err,
        }
    }
}

impl Write for FailAfterWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.pos >= BODY_OFFSET && self.pos + buf.len() as u64 > self.body_region_limit {
            return Err(io::Error::new(self.err.kind(), self.err.to_string()));
        }
        let n = self.inner.write(buf)?;
        self.pos += n as u64;
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

impl Seek for FailAfterWriter {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        self.pos = self.inner.seek(pos)?;
        Ok(self.pos)
    }
}

/// A write target whose next `flush` fails once; every other operation (writes
/// and later flushes) succeeds. Models a transient write failure at finalize
/// time, so the recorder proves it patches the header anyway.
#[derive(Debug)]
pub struct FlushFailOnceWriter {
    inner: File,
    err: io::Error,
    fail_next_flush: bool,
}

impl FlushFailOnceWriter {
    pub fn new(inner: File, err: io::Error) -> Self {
        Self {
            inner,
            err,
            fail_next_flush: true,
        }
    }
}

impl Write for FlushFailOnceWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.inner.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        if self.fail_next_flush {
            self.fail_next_flush = false;
            return Err(io::Error::new(self.err.kind(), self.err.to_string()));
        }
        self.inner.flush()
    }
}

impl Seek for FlushFailOnceWriter {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        self.inner.seek(pos)
    }
}
