//! Private bounded preparation files. Publication consumes records, not source handles.
use anyhow::{Context, Result, ensure};
use llm_history_types::ingestion::PreparedRecord;
use std::{
    fs::File,
    io::{BufRead, BufReader, BufWriter, Read, Seek, SeekFrom, Write},
};

const MAX_RAW: u64 = 64 * 1024 * 1024;
const MAX_METADATA: u64 = 256 * 1024 * 1024;
const MAX_SPOOL: u64 = 4 * 1024 * 1024 * 1024;

pub(super) struct Spool {
    writer: BufWriter<File>,
    size: u64,
}

impl Spool {
    pub(super) fn new() -> Result<Self> {
        Ok(Self {
            writer: BufWriter::new(tempfile::tempfile()?),
            size: 0,
        })
    }

    pub(super) fn push(&mut self, mut record: PreparedRecord) -> Result<()> {
        let raw = record
            .raw
            .as_mut()
            .map(|r| std::mem::take(&mut r.bytes))
            .unwrap_or_default();
        ensure!(
            raw.len() as u64 <= MAX_RAW,
            "prepared raw record exceeds the capture limit"
        );
        let mut bounded = Limited {
            writer: &mut self.writer,
            remaining: MAX_SPOOL - self.size,
            frame_remaining: u64::MAX,
        };
        bounded.write_all(&(raw.len() as u64).to_le_bytes())?;
        bounded.write_all(&raw)?;
        bounded.frame_remaining = MAX_METADATA;
        serde_json::to_writer(&mut bounded, &record)?;
        bounded.write_all(b"\n")?;
        self.size = MAX_SPOOL - bounded.remaining;
        Ok(())
    }

    pub(super) fn finish(mut self) -> Result<Prepared> {
        self.writer.flush()?;
        Ok(Prepared {
            file: self.writer.into_inner()?,
            size: self.size,
        })
    }
}

struct Limited<'a> {
    writer: &'a mut BufWriter<File>,
    remaining: u64,
    frame_remaining: u64,
}

impl Write for Limited<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() as u64 > self.frame_remaining {
            return Err(std::io::Error::other(
                "prepared record metadata exceeds the spool frame limit",
            ));
        }
        if bytes.len() as u64 > self.remaining {
            return Err(std::io::Error::other(
                "source preparation exceeds the spool limit",
            ));
        }
        let written = self.writer.write(bytes)?;
        self.remaining -= written as u64;
        self.frame_remaining -= written as u64;
        Ok(written)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.writer.flush()
    }
}

pub(super) struct Prepared {
    file: File,
    size: u64,
}

impl Prepared {
    pub(super) fn size(&self) -> u64 {
        self.size
    }

    pub(super) fn records(&self) -> Result<impl Iterator<Item = Result<PreparedRecord>>> {
        let mut file = self.file.try_clone()?;
        file.seek(SeekFrom::Start(0))?;
        let mut reader = BufReader::new(file.take(self.size));
        Ok(std::iter::from_fn(move || {
            let ready = match reader.fill_buf() {
                Ok(bytes) => !bytes.is_empty(),
                Err(error) => return Some(Err(error.into())),
            };
            if !ready {
                return None;
            }
            Some((|| {
                let mut size = [0; 8];
                reader.read_exact(&mut size)?;
                let size = u64::from_le_bytes(size);
                ensure!(size <= MAX_RAW, "invalid prepared record length");
                let mut bytes = vec![0; size as usize];
                reader.read_exact(&mut bytes)?;
                let mut metadata = Vec::new();
                reader
                    .by_ref()
                    .take(MAX_METADATA + 1)
                    .read_until(b'\n', &mut metadata)?;
                ensure!(
                    metadata.len() as u64 <= MAX_METADATA && metadata.last() == Some(&b'\n'),
                    "invalid prepared record metadata"
                );
                let mut record: PreparedRecord = serde_json::from_slice(&metadata)?;
                if !bytes.is_empty() {
                    record
                        .raw
                        .as_mut()
                        .context("unexpected prepared raw bytes")?
                        .bytes = bytes;
                }
                Ok(record)
            })())
        }))
    }
}
