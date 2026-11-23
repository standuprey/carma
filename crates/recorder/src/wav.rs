//! 24-bit PCM WAV/RIFF writer.

use std::fs::File;
use std::io::{BufWriter, Seek, SeekFrom, Write};
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
#[derive(Debug)]
pub struct WavRecorder {
    writer: BufWriter<File>,
    channels: u16,
    body_bytes: u32,
    finalized: bool,
}

impl WavRecorder {
    pub fn open(path: &Path, sample_rate: u32, channels: u16) -> Result<Self, RecorderError> {
        let file = File::create(path).map_err(|e| RecorderError::Io(e.to_string()))?;
        let mut writer = BufWriter::new(file);
        let block_align = channels * BITS_PER_SAMPLE / 8;
        let byte_rate = sample_rate * u32::from(block_align);

        // Placeholder header; sizes patched at finalize.
        writer.write_all(RIFF).map_err(|e| RecorderError::Io(e.to_string()))?;
        writer.write_all(&0u32.to_le_bytes()).map_err(|e| RecorderError::Io(e.to_string()))?;
        writer.write_all(WAVE).map_err(|e| RecorderError::Io(e.to_string()))?;
        writer.write_all(FMT).map_err(|e| RecorderError::Io(e.to_string()))?;
        writer.write_all(&16u32.to_le_bytes()).map_err(|e| RecorderError::Io(e.to_string()))?;
        writer.write_all(&PCM.to_le_bytes()).map_err(|e| RecorderError::Io(e.to_string()))?;
        writer.write_all(&channels.to_le_bytes()).map_err(|e| RecorderError::Io(e.to_string()))?;
        writer.write_all(&sample_rate.to_le_bytes()).map_err(|e| RecorderError::Io(e.to_string()))?;
        writer.write_all(&byte_rate.to_le_bytes()).map_err(|e| RecorderError::Io(e.to_string()))?;
        writer.write_all(&block_align.to_le_bytes()).map_err(|e| RecorderError::Io(e.to_string()))?;
        writer.write_all(&BITS_PER_SAMPLE.to_le_bytes()).map_err(|e| RecorderError::Io(e.to_string()))?;
        writer.write_all(DATA).map_err(|e| RecorderError::Io(e.to_string()))?;
        writer.write_all(&0u32.to_le_bytes()).map_err(|e| RecorderError::Io(e.to_string()))?;

        Ok(Self {
            writer,
            channels,
            body_bytes: 0,
            finalized: false,
        })
    }
}

impl Recorder for WavRecorder {
    fn push(&mut self, buf: &[f32]) -> Result<(), RecorderError> {
        if self.finalized {
            return Err(RecorderError::AlreadyFinalized);
        }
        if buf.len() % usize::from(self.channels) != 0 {
            return Err(RecorderError::Misaligned);
        }
        for &sample in buf {
            let scaled = (f64::from(sample).clamp(-1.0, 1.0) * 8_388_607.0) as i32;
            let bytes = scaled.to_le_bytes();
            self.writer
                .write_all(&bytes[..3])
                .map_err(|e| RecorderError::Io(e.to_string()))?;
            self.body_bytes += BYTES_PER_SAMPLE;
        }
        Ok(())
    }

    fn finalize(&mut self) -> Result<(), RecorderError> {
        if self.finalized {
            return Err(RecorderError::AlreadyFinalized);
        }
        self.writer.flush().map_err(|e| RecorderError::Io(e.to_string()))?;
        let riff_size = 36 + self.body_bytes;
        let stream = self.writer.get_mut();
        stream.seek(SeekFrom::Start(OFFSET_RIFF_SIZE)).map_err(|e| RecorderError::Io(e.to_string()))?;
        stream.write_all(&riff_size.to_le_bytes()).map_err(|e| RecorderError::Io(e.to_string()))?;
        stream.seek(SeekFrom::Start(OFFSET_DATA_SIZE)).map_err(|e| RecorderError::Io(e.to_string()))?;
        stream.write_all(&self.body_bytes.to_le_bytes()).map_err(|e| RecorderError::Io(e.to_string()))?;
        stream.flush().map_err(|e| RecorderError::Io(e.to_string()))?;
        self.finalized = true;
        Ok(())
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
    use interfaces::Recorder;

    #[test]
    fn header_asserts_24bit_pcm_48k_stereo_and_frame_count() {
        let path = std::env::temp_dir().join(format!(
            "carma_wav_test_{}.wav",
            std::process::id()
        ));
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
        let path = std::env::temp_dir().join(format!(
            "carma_wav_finalize_{}.wav",
            std::process::id()
        ));
        let mut r = WavRecorder::open(&path, 48_000, 2).unwrap();
        r.finalize().unwrap();
        assert!(matches!(r.finalize(), Err(RecorderError::AlreadyFinalized)));
        std::fs::remove_file(&path).ok();
    }
}