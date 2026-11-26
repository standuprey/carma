//! 24-bit PCM WAV/RIFF writer.

use std::fmt::Debug;
use std::fs::File;
use std::io::{self, Seek, SeekFrom, Write};
use std::path::Path;

use interfaces::{Recorder, RecorderError};

const RIFF: &[u8; 4] = b"RIFF";
const WAVE: &[u8; 4] = b"WAVE";
const FMT: &[u8; 4] = b"fmt ";
const DATA: &[u8; 4] = b"data";
const PCM: u16 = 1;
const BITS_PER_SAMPLE: u16 = 24;
const BYTES_PER_SAMPLE: u32 = 3;
/// Offset of the RIFF chunk-size field.
const OFFSET_RIFF_SIZE: u64 = 4;
/// Offset of the data-chunk byte-count field (fixed at open).
const OFFSET_DATA_SIZE: u64 = 40;

/// Size of the canonical 44-byte WAV header before the data chunk begins.
const HEADER_BYTES: u64 = 44;

/// A seekable write target for the recorder. Internal union trait so the
/// writer can be a single trait object with `Send`/`Debug`.
trait WriteSeek: Write + Seek + Debug {}
impl<T: Write + Seek + Debug> WriteSeek for T {}

/// Parsed facts about a WAV file header, used to assert format correctness.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WavHeader {
    pub format: u16,
    pub channels: u16,
    pub sample_rate: u32,
    pub bits_per_sample: u16,
    /// Number of frames in the data chunk.
    pub num_frames: u32,
}

/// A WAV recorder. Writes a placeholder header at open, streams interleaved
/// 24-bit PCM, and rewrites the sizes on [`Recorder::finalize`].
///
/// Body bytes are written to the target in one block per push and counted at
/// the exact point they land, which is what keeps the finalized header
/// consistent with the bytes actually written even when a take fails
/// mid-write (e.g. disk full): on a partial write failure the seekable
/// writer's position reports how many bytes were accepted.
#[derive(Debug)]
pub struct WavRecorder {
    writer: Box<dyn WriteSeek + Send>,
    /// Reused scratch buffer for encoding one pushed chunk.
    scratch: Vec<u8>,
    channels: u16,
    body_bytes: u32,
    finalized: bool,
}

impl WavRecorder {
    pub fn open(path: &Path, sample_rate: u32, channels: u16) -> Result<Self, RecorderError> {
        let file = File::create(path).map_err(|e| RecorderError::Io(e.to_string()))?;
        Self::from_writer(file, sample_rate, channels)
    }

    /// Opens a recorder over an arbitrary write target, so tests can inject
    /// failing writers and model write-error/disk-full behavior.
    pub fn from_writer<W: Write + Seek + Send + Debug + 'static>(
        writer: W,
        sample_rate: u32,
        channels: u16,
    ) -> Result<Self, RecorderError> {
        let mut writer: Box<dyn WriteSeek + Send> = Box::new(writer);
        let block_align = channels * BITS_PER_SAMPLE / 8;
        let byte_rate = sample_rate * u32::from(block_align);

        // Placeholder header; sizes patched at finalize.
        writer.write_all(RIFF).map_err(map_io_err)?;
        writer.write_all(&0u32.to_le_bytes()).map_err(map_io_err)?;
        writer.write_all(WAVE).map_err(map_io_err)?;
        writer.write_all(FMT).map_err(map_io_err)?;
        writer.write_all(&16u32.to_le_bytes()).map_err(map_io_err)?;
        writer.write_all(&PCM.to_le_bytes()).map_err(map_io_err)?;
        writer
            .write_all(&channels.to_le_bytes())
            .map_err(map_io_err)?;
        writer
            .write_all(&sample_rate.to_le_bytes())
            .map_err(map_io_err)?;
        writer
            .write_all(&byte_rate.to_le_bytes())
            .map_err(map_io_err)?;
        writer
            .write_all(&block_align.to_le_bytes())
            .map_err(map_io_err)?;
        writer
            .write_all(&BITS_PER_SAMPLE.to_le_bytes())
            .map_err(map_io_err)?;
        writer.write_all(DATA).map_err(map_io_err)?;
        writer.write_all(&0u32.to_le_bytes()).map_err(map_io_err)?;

        Ok(Self {
            writer,
            scratch: Vec::new(),
            channels,
            body_bytes: 0,
            finalized: false,
        })
    }
}

fn map_io_err(e: io::Error) -> RecorderError {
    if e.kind() == io::ErrorKind::StorageFull {
        RecorderError::DiskFull(e.to_string())
    } else {
        RecorderError::Io(e.to_string())
    }
}

impl Recorder for WavRecorder {
    fn push(&mut self, buf: &[f32]) -> Result<(), RecorderError> {
        if self.finalized {
            return Err(RecorderError::AlreadyFinalized);
        }
        if !buf.len().is_multiple_of(usize::from(self.channels)) {
            return Err(RecorderError::Misaligned);
        }
        // Encode the whole chunk first, then write it as one block: this
        // keeps the write-syscall rate bounded by the push rate (~47/s at the
        // 1024-frame quantum) instead of one 3-byte write per sample (~96k/s).
        self.scratch.clear();
        self.scratch.reserve(buf.len() * BYTES_PER_SAMPLE as usize);
        for &sample in buf {
            let scaled = (f64::from(sample).clamp(-1.0, 1.0) * 8_388_607.0) as i32;
            self.scratch.extend_from_slice(&scaled.to_le_bytes()[..3]);
        }
        match self.writer.write_all(&self.scratch) {
            Ok(()) => {
                self.body_bytes += self.scratch.len() as u32;
                Ok(())
            }
            Err(e) => {
                // A real disk-full can accept part of a block before failing;
                // count exactly the bytes that landed via the writer position
                // so `frames_written` and the finalized header match the file.
                if let Ok(pos) = self.writer.stream_position() {
                    self.body_bytes = pos.saturating_sub(HEADER_BYTES) as u32;
                }
                Err(map_io_err(e))
            }
        }
    }

    fn finalize(&mut self) -> Result<(), RecorderError> {
        if self.finalized {
            return Err(RecorderError::AlreadyFinalized);
        }
        // Flush whatever the writer still buffers, but keep going on failure:
        // the sizes below are what make the file parseable, so a failed flush
        // (e.g. disk full) must not skip them. Real filesystems usually still
        // accept the small in-place size patch because the header blocks were
        // allocated when the file was created.
        let flush_problem = self.writer.flush().err();
        let patched = (|| -> Result<(), RecorderError> {
            let riff_size = 36 + self.body_bytes;
            self.writer
                .seek(SeekFrom::Start(OFFSET_RIFF_SIZE))
                .map_err(map_io_err)?;
            self.writer
                .write_all(&riff_size.to_le_bytes())
                .map_err(map_io_err)?;
            self.writer
                .seek(SeekFrom::Start(OFFSET_DATA_SIZE))
                .map_err(map_io_err)?;
            self.writer
                .write_all(&self.body_bytes.to_le_bytes())
                .map_err(map_io_err)?;
            self.writer.flush().map_err(map_io_err)?;
            Ok(())
        })();
        match patched {
            Ok(()) => {
                self.finalized = true;
                if let Some(e) = flush_problem {
                    return Err(map_io_err(e));
                }
                Ok(())
            }
            Err(e) => Err(e),
        }
    }

    fn frames_written(&self) -> u64 {
        u64::from(self.body_bytes) / (u64::from(self.channels) * u64::from(BYTES_PER_SAMPLE))
    }
}

/// Reads and parses a WAV file written by `WavRecorder` so tests can assert
/// the exact header. Only the leading fields this recorder controls are read.
pub fn read_wav_header(path: &Path) -> Result<WavHeader, String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    if bytes.len() < 44 {
        return Err("file too short to be a WAV header".into());
    }
    if &bytes[0..4] != RIFF || &bytes[8..12] != WAVE || &bytes[12..16] != FMT {
        return Err("missing RIFF/WAVE/fmt markers".into());
    }
    if &bytes[36..40] != DATA {
        return Err("missing data marker".into());
    }
    let b = |o: usize| bytes[o];
    let u16le = |o: usize| u16::from_le_bytes([b(o), b(o + 1)]);
    let u32le = |o: usize| u32::from_le_bytes([b(o), b(o + 1), b(o + 2), b(o + 3)]);
    let data_size = u32le(40);
    let block_align = u16le(32);
    Ok(WavHeader {
        format: u16le(20),
        channels: u16le(22),
        sample_rate: u32le(24),
        bits_per_sample: u16le(34),
        num_frames: data_size / u32::from(block_align),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{FailAfterWriter, FlushFailOnceWriter};
    use interfaces::Recorder;

    fn temp_path(label: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("carma_{label}_{}.wav", std::process::id()))
    }

    #[test]
    fn header_asserts_24bit_pcm_48k_stereo_and_frame_count() {
        let path = temp_path("wav_ok");
        let mut r = WavRecorder::open(&path, 48_000, 2).unwrap();
        const FRAMES: usize = 960;
        for _ in 0..FRAMES {
            r.push(&[-0.5, 0.5]).unwrap();
        }
        r.finalize().unwrap();

        let h = read_wav_header(&path).unwrap();
        assert_eq!(h.format, 1, "PCM format tag");
        assert_eq!(h.bits_per_sample, 24, "bits per sample");
        assert_eq!(h.sample_rate, 48_000, "sample rate");
        assert_eq!(h.channels, 2, "channels");
        assert_eq!(h.num_frames as usize, FRAMES, "frame count");

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn finalize_refuses_twice() {
        let path = temp_path("wav_finalize");
        let mut r = WavRecorder::open(&path, 48_000, 2).unwrap();
        r.finalize().unwrap();
        assert!(matches!(r.finalize(), Err(RecorderError::AlreadyFinalized)));
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn write_error_yields_valid_partial_wav_with_exact_header() {
        let path = temp_path("wav_partial");
        let file = File::create(&path).unwrap();
        let no_space = io::Error::new(io::ErrorKind::StorageFull, "no space left on device");
        // Budget of 300 stereo frames: 300 * 2ch * 3 bytes.
        let failing = FailAfterWriter::new(file, 300 * 2 * 3, no_space);
        let mut r = WavRecorder::from_writer(failing, 48_000, 2).unwrap();

        let err = (0..2000)
            .map(|_| r.push(&[-0.5, 0.5]))
            .find(|r| r.is_err())
            .expect("writer must run out of space");

        assert!(
            matches!(err, Err(RecorderError::DiskFull(_))),
            "disk-full must be reported, got {err:?}"
        );
        assert_eq!(r.frames_written(), 300, "only accepted frames count");

        r.finalize().unwrap();
        let h = read_wav_header(&path).unwrap();
        assert_eq!(
            h.num_frames, 300,
            "header must match bytes actually written"
        );

        let meta = std::fs::metadata(&path).unwrap();
        assert_eq!(
            meta.len(),
            44 + 300 * 2 * 3,
            "file must be exactly header + written body"
        );

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn write_error_at_first_sample_leaves_valid_empty_wav() {
        let path = temp_path("wav_empty");
        let file = File::create(&path).unwrap();
        let no_space = io::Error::new(io::ErrorKind::StorageFull, "no space left on device");
        // No body bytes accepted at all: the take fails on its first write.
        let failing = FailAfterWriter::new(file, 0, no_space);
        let mut r = WavRecorder::from_writer(failing, 48_000, 2).unwrap();

        assert!(matches!(
            r.push(&[0.0, 0.0]),
            Err(RecorderError::DiskFull(_))
        ));
        assert_eq!(r.frames_written(), 0);

        r.finalize().unwrap();
        let h = read_wav_header(&path).unwrap();
        assert_eq!(h.num_frames, 0, "empty take is a valid empty WAV");
        assert_eq!(std::fs::metadata(&path).unwrap().len(), 44);

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn finalize_reports_flush_failure_but_still_patches_header() {
        let path = temp_path("wav_flush_fail");
        let file = File::create(&path).unwrap();
        let no_space = io::Error::new(io::ErrorKind::StorageFull, "no space left on device");
        let failing = FlushFailOnceWriter::new(file, no_space);
        let mut r = WavRecorder::from_writer(failing, 48_000, 2).unwrap();

        for _ in 0..10 {
            r.push(&[-0.5, 0.5]).unwrap();
        }
        // The failed finalize flush surfaces, but the header must still be
        // patched so the partial file is valid, not truncated/mismatched.
        let err = r.finalize().unwrap_err();
        assert!(matches!(err, RecorderError::DiskFull(_)), "got {err:?}");

        let h = read_wav_header(&path).unwrap();
        assert_eq!(h.num_frames, 10, "header patched even after flush failure");

        std::fs::remove_file(&path).ok();
    }
}
