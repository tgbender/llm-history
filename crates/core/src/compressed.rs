//! Codex compresses finished rollouts to `.jsonl.zst`. They are read through a pure-Rust
//! decoder and identified by their `.jsonl` name, so compression continues a source.
use ruzstd::decoding::errors::{FrameDecoderError, ReadFrameHeaderError};
use ruzstd::decoding::{BlockDecodingStrategy, FrameDecoder};
use std::io::{self, BufRead, BufReader, Read};
use std::path::{Path, PathBuf};

pub fn is_compressed(path: &Path) -> bool {
    path.extension().is_some_and(|e| e == "zst")
        && path
            .file_stem()
            .is_some_and(|s| s.to_string_lossy().ends_with(".jsonl"))
}
/// The plain `.jsonl` path a source is known by.
pub fn logical_path(path: &Path) -> PathBuf {
    if is_compressed(path) {
        path.with_extension("")
    } else {
        path.to_owned()
    }
}

/// Decodes every frame of a zstd file in order, skipping skippable frames and checking
/// each frame's content checksum when it has one.
pub(crate) struct ZstdStream<R> {
    input: BufReader<R>,
    frame: FrameDecoder,
    active: bool,
}
impl<R: Read> ZstdStream<R> {
    pub(crate) fn new(input: R) -> Self {
        Self {
            input: BufReader::new(input),
            frame: FrameDecoder::new(),
            active: false,
        }
    }
}
impl<R: Read> Read for ZstdStream<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        loop {
            if self.active {
                while self.frame.can_collect() == 0 && !self.frame.is_finished() {
                    self.frame
                        .decode_blocks(&mut self.input, BlockDecodingStrategy::UptoBlocks(1))
                        .map_err(io::Error::other)?;
                }
                let n = self.frame.read(buf)?;
                if n > 0 {
                    return Ok(n);
                }
                if let Some(expected) = self.frame.get_checksum_from_data()
                    && self.frame.get_calculated_checksum() != Some(expected)
                {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "zstd frame checksum mismatch",
                    ));
                }
                self.active = false;
            }
            if self.input.fill_buf()?.is_empty() {
                return Ok(0);
            }
            match self.frame.reset(&mut self.input) {
                Ok(()) => self.active = true,
                Err(FrameDecoderError::ReadFrameHeaderError(ReadFrameHeaderError::SkipFrame {
                    length,
                    ..
                })) => {
                    let skipped = io::copy(
                        &mut (&mut self.input).take(u64::from(length)),
                        &mut io::sink(),
                    )?;
                    if skipped != u64::from(length) {
                        return Err(io::ErrorKind::UnexpectedEof.into());
                    }
                }
                Err(e) => return Err(io::Error::other(e)),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    #[test]
    fn names_map_to_their_plain_source() {
        let p = Path::new("sessions/a.jsonl.zst");
        assert!(is_compressed(p));
        assert_eq!(logical_path(p), Path::new("sessions/a.jsonl"));
        assert!(!is_compressed(Path::new("notes.zst")));
        assert_eq!(logical_path(Path::new("a.jsonl")), Path::new("a.jsonl"));
    }
    #[test]
    fn decodes_multiple_frames_and_skips_skippable_ones() -> anyhow::Result<()> {
        let mut f = tempfile::NamedTempFile::new()?;
        let frame = |text: &str| {
            ruzstd::encoding::compress_to_vec(
                text.as_bytes(),
                ruzstd::encoding::CompressionLevel::Fastest,
            )
        };
        f.write_all(&frame("{\"a\":1}\n"))?;
        // Skippable frame: magic 0x184D2A50, little-endian length, payload.
        f.write_all(&[0x50, 0x2A, 0x4D, 0x18, 3, 0, 0, 0, 9, 9, 9])?;
        f.write_all(&frame("{\"b\":2}\n"))?;
        let mut out = String::new();
        ZstdStream::new(std::fs::File::open(f.path())?).read_to_string(&mut out)?;
        assert_eq!(out, "{\"a\":1}\n{\"b\":2}\n");
        Ok(())
    }
}
