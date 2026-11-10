//! Source capture, timestamp interpretation and shared output policy.
use anyhow::{Context, Result, bail};
use chrono::{DateTime, NaiveDate, NaiveDateTime, TimeZone, Utc};
use regex::Regex;
use serde_json::{Value, json};
use std::fs::{File, Metadata};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

mod compressed;
pub mod json_string;
pub mod paths;
pub mod provider;
pub mod visibility;
pub use compressed::{is_compressed, logical_path};
/// Longest session title, in characters.
pub const TITLE_CHARS: usize = 120;

use llm_history_types::{Fact, Harness};

/// Searchable text under the established guardian visibility policy.
pub fn fact_search_text(fact: &Fact) -> &str {
    fact.guardian_prompt
        .then(|| guardian_action(&fact.text))
        .flatten()
        .unwrap_or(&fact.text)
}

/// The action block of a Codex guardian prompt. It follows the transcript, so the last block
/// is the real one even if the transcript quotes an earlier prompt. Markers must fill their
/// line: action JSON escapes newlines, so a marker quoted in a command can't end the block
/// early. Reviews of native tool calls have no block.
pub fn guardian_action(text: &str) -> Option<&str> {
    const START: &str = ">>> APPROVAL REQUEST START";
    const END: &str = ">>> APPROVAL REQUEST END";
    fn last_line(text: &str, marker: &str) -> Option<usize> {
        text.rmatch_indices(marker).map(|(at, _)| at).find(|&at| {
            (at == 0 || text[..at].ends_with('\n'))
                && matches!(text[at + marker.len()..].chars().next(), None | Some('\n'))
        })
    }
    let end = last_line(text, END)?;
    let start = last_line(&text[..end], START)?;
    let action = text[start + START.len()..end].trim();
    (!action.is_empty()).then_some(action)
}

#[derive(Debug)]
pub struct RawRecord {
    pub ordinal: u64,
    pub start: u64,
    pub end: u64,
    pub bytes: Vec<u8>,
    pub value: Option<Value>,
}

/// Bytes kept from the start and from just before the committed offset. Appends are
/// accepted only when both windows still match; unchanged size and mtime skip reads.
pub const WINDOW: usize = 64 * 1024;

/// Reads a bounded prefix from one open descriptor, publishing only complete lines.
pub struct Capture {
    reader: Box<dyn BufRead>,
    path: PathBuf,
    pub initial: Metadata,
    pub committed: u64,
    pub ordinal: u64,
    head: Vec<u8>,
    tail: Vec<u8>,
    max_record: usize,
    /// Whether the observed decoded snapshot ended on a complete JSONL record.
    pub complete: bool,
}
impl Capture {
    /// `head` and `tail` are the validated windows of the already committed prefix.
    pub fn open(
        path: &Path,
        offset: u64,
        ordinal: u64,
        max_record: usize,
        windows: (Vec<u8>, Vec<u8>),
    ) -> Result<Self> {
        let mut file = File::open(path).with_context(|| format!("read {}", path.display()))?;
        let initial = file.metadata()?;
        let reader: Box<dyn BufRead> = if is_compressed(path) {
            let mut stream = BufReader::new(compressed::ZstdStream::new(file.take(initial.len())));
            let skipped = std::io::copy(&mut (&mut stream).take(offset), &mut std::io::sink())?;
            if skipped != offset {
                bail!("source was truncated");
            }
            Box::new(stream)
        } else {
            if offset > initial.len() {
                bail!("source was truncated");
            }
            file.seek(SeekFrom::Start(offset))?;
            Box::new(BufReader::new(file.take(initial.len() - offset)))
        };
        Ok(Self::new(
            path, reader, initial, offset, ordinal, max_record, windows,
        ))
    }
    /// Continues after `committed` bytes when that prefix's head and boundary windows
    /// still hash to `expected`, or returns `None`. A compressed source is decoded once
    /// for both the check and the capture.
    pub fn resume(
        path: &Path,
        committed: u64,
        ordinal: u64,
        max_record: usize,
        expected: (u64, u64),
    ) -> Result<Option<Self>> {
        if !is_compressed(path) {
            if std::fs::metadata(path)?.len() < committed {
                return Ok(None);
            }
            let windows = read_windows(path, committed)?;
            if window_hashes(&windows.0, &windows.1) != expected {
                return Ok(None);
            }
            return Self::open(path, committed, ordinal, max_record, windows).map(Some);
        }
        let file = File::open(path).with_context(|| format!("read {}", path.display()))?;
        let initial = file.metadata()?;
        let mut stream = BufReader::new(compressed::ZstdStream::new(file.take(initial.len())));
        // A decoded snapshot shorter than the checkpoint, or undecodable, is a rewrite.
        let Ok(windows) = windows_from(&mut stream, committed) else {
            return Ok(None);
        };
        if window_hashes(&windows.0, &windows.1) != expected {
            return Ok(None);
        }
        Ok(Some(Self::new(
            path,
            Box::new(stream),
            initial,
            committed,
            ordinal,
            max_record,
            windows,
        )))
    }
    fn new(
        path: &Path,
        reader: Box<dyn BufRead>,
        initial: Metadata,
        committed: u64,
        ordinal: u64,
        max_record: usize,
        (head, tail): (Vec<u8>, Vec<u8>),
    ) -> Self {
        Self {
            reader,
            path: path.to_owned(),
            initial,
            committed,
            ordinal,
            head,
            tail,
            max_record,
            complete: true,
        }
    }
    pub fn next_record(&mut self) -> Result<Option<RawRecord>> {
        let mut bytes = Vec::new();
        // take bounds even a malicious newline-free record before allocating it all.
        self.reader
            .by_ref()
            .take(self.max_record as u64 + 1)
            .read_until(b'\n', &mut bytes)?;
        if bytes.len() > self.max_record {
            bail!("record exceeds {} bytes", self.max_record);
        }
        if bytes.is_empty() {
            return Ok(None);
        }
        if bytes.last() != Some(&b'\n') {
            self.complete = false;
            return Ok(None);
        }
        let start = self.committed;
        self.committed += bytes.len() as u64;
        self.ordinal += 1;
        if self.head.len() < WINDOW {
            let n = (WINDOW - self.head.len()).min(bytes.len());
            self.head.extend_from_slice(&bytes[..n]);
        }
        self.tail.extend_from_slice(&bytes);
        if self.tail.len() > 2 * WINDOW {
            self.tail.drain(..self.tail.len() - WINDOW);
        }
        let value = serde_json::from_slice::<Value>(&bytes)
            .ok()
            .filter(Value::is_object);
        Ok(Some(RawRecord {
            ordinal: self.ordinal,
            start,
            end: self.committed,
            bytes,
            value,
        }))
    }
    /// Rejects a source replaced or truncated while it was read; content checks happen
    /// through the windows on the next update.
    pub fn check_source(&self) -> Result<()> {
        let current = std::fs::metadata(&self.path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if current.ino() != self.initial.ino() || current.dev() != self.initial.dev() {
                bail!("source was replaced during capture");
            }
        }
        // Compressed sources are whole files; plain ones may only grow.
        if is_compressed(&self.path) && current.len() != self.initial.len()
            || !is_compressed(&self.path) && current.len() < self.committed
        {
            bail!("source changed during capture");
        }
        Ok(())
    }
    /// xxh64 of the captured head and boundary windows.
    pub fn window_hashes(&self) -> (u64, u64) {
        window_hashes(&self.head, &self.tail)
    }
}
/// Reads the head and boundary windows of the first `committed` bytes of a file.
/// Compressed sources are decoded up to `committed`; plain ones read only the windows.
pub fn read_windows(path: &Path, committed: u64) -> Result<(Vec<u8>, Vec<u8>)> {
    if is_compressed(path) {
        return windows_from(compressed::ZstdStream::new(File::open(path)?), committed);
    }
    let size = committed.min(WINDOW as u64);
    let mut head = vec![0; size as usize];
    let mut tail = vec![0; size as usize];
    let mut file = File::open(path)?;
    file.read_exact(&mut head)?;
    file.seek(SeekFrom::Start(committed - size))?;
    file.read_exact(&mut tail)?;
    Ok((head, tail))
}
/// Reads the windows of the first `committed` bytes of a stream, leaving it there.
fn windows_from(mut stream: impl Read, committed: u64) -> Result<(Vec<u8>, Vec<u8>)> {
    let size = committed.min(WINDOW as u64);
    let mut head = vec![0; size as usize];
    stream.read_exact(&mut head)?;
    // Tail bytes not already in the head follow after skipping the middle.
    let after_head = committed - size;
    let fresh = after_head.min(size);
    let skip = after_head - fresh;
    if std::io::copy(&mut (&mut stream).take(skip), &mut std::io::sink())? != skip {
        bail!("source was truncated");
    }
    let mut last = vec![0; fresh as usize];
    stream.read_exact(&mut last)?;
    let tail = [&head[fresh as usize..], last.as_slice()].concat();
    Ok((head, tail))
}
/// Change-detection hashes of a head window and the last `WINDOW` bytes of a tail.
pub fn window_hashes(head: &[u8], tail: &[u8]) -> (u64, u64) {
    let tail = &tail[tail.len().saturating_sub(WINDOW)..];
    (
        xxhash_rust::xxh64::xxh64(head, 0),
        xxhash_rust::xxh64::xxh64(tail, 0),
    )
}
pub fn mtime_ns(metadata: &Metadata) -> i64 {
    metadata
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_nanos() as i64)
}
/// Full-prefix hash, used only to adopt stores written before window checkpoints.
pub fn hash_range(path: &Path, offset: u64, size: u64) -> Result<String> {
    let mut source: Box<dyn Read> = if is_compressed(path) {
        Box::new(compressed::ZstdStream::new(File::open(path)?))
    } else {
        Box::new(File::open(path)?)
    };
    std::io::copy(&mut (&mut source).take(offset), &mut std::io::sink())?;
    let mut reader = source.take(size);
    let mut hasher = blake3::Hasher::new();
    let mut buf = [0; 65536];
    let mut seen = 0;
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        seen += n as u64;
        hasher.update(&buf[..n]);
    }
    if seen != size {
        bail!("source prefix changed length");
    }
    Ok(hasher.finalize().to_hex().to_string())
}
pub fn timestamp(value: &str, zone: &str) -> Result<String> {
    if zone.eq_ignore_ascii_case("local") {
        if let Ok(time) = DateTime::parse_from_rfc3339(value.trim()) {
            return Ok(time.with_timezone(&Utc).to_rfc3339());
        }
        let naive = provider::parse_naive(value.trim()).context("invalid ISO date or timestamp")?;
        return Ok(chrono::Local
            .from_local_datetime(&naive)
            .earliest()
            .context("nonexistent local timestamp")?
            .with_timezone(&Utc)
            .to_rfc3339());
    }
    provider::timestamp(value, zone)
        .utc
        .with_context(|| format!("invalid date/timestamp or timezone: {value} ({zone})"))
}
pub fn project_name(cwd: &str) -> String {
    cwd.replace('\\', "/")
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .to_owned()
}
pub fn code_aliases(text: &str) -> String {
    static CANDIDATES: OnceLock<Regex> = OnceLock::new();
    static SIGNAL: OnceLock<Regex> = OnceLock::new();
    static DELIMITERS: OnceLock<Regex> = OnceLock::new();
    fn folded(s: &str) -> String {
        s.to_lowercase()
            .replace('ß', "ss")
            .replace('ς', "σ")
            .replace('ﬀ', "ff")
            .replace('ﬁ', "fi")
            .replace('ﬂ', "fl")
            .replace('ﬃ', "ffi")
            .replace('ﬄ', "ffl")
    }
    fn add(out: &mut Vec<String>, s: &str) {
        let s = folded(s).trim_matches('_').to_owned();
        if (2..=256).contains(&s.chars().count()) && !out.contains(&s) {
            out.push(s);
        }
    }
    fn parts(s: &str, numbers: bool) -> Vec<&str> {
        let chars = s.char_indices().collect::<Vec<_>>();
        let mut starts = vec![0];
        for i in 1..chars.len() {
            let a = chars[i - 1].1;
            let b = chars[i].1;
            let boundary = if numbers {
                (a.is_ascii_alphabetic() && b.is_ascii_digit())
                    || (a.is_ascii_digit() && b.is_ascii_alphabetic())
            } else {
                ((a.is_ascii_lowercase() || a.is_ascii_digit()) && b.is_ascii_uppercase())
                    || (a.is_ascii_uppercase()
                        && b.is_ascii_uppercase()
                        && chars
                            .get(i + 1)
                            .is_some_and(|(_, c)| c.is_ascii_lowercase()))
            };
            if boundary {
                starts.push(chars[i].0);
            }
        }
        starts.push(s.len());
        starts.windows(2).map(|w| &s[w[0]..w[1]]).collect()
    }
    let candidates = CANDIDATES.get_or_init(|| Regex::new(r"[\p{L}\p{N}_.+#:/\\-]{2,}").unwrap());
    let signal = SIGNAL.get_or_init(|| {
        Regex::new(r"[a-z0-9][A-Z]|[A-Z]{2}[a-z]|[A-Za-z][0-9]|[0-9][A-Za-z]").unwrap()
    });
    let delimiters = DELIMITERS.get_or_init(|| Regex::new(r"[^\p{L}\p{N}]+").unwrap());
    let mut aliases = Vec::new();
    for found in candidates.find_iter(text) {
        let token = found
            .as_str()
            .trim_matches(|c| ".,;:!?()[]{}<>\"'`".contains(c));
        if token.is_empty() || token.chars().count() > 256 {
            continue;
        }
        if !token.chars().any(|c| !c.is_alphanumeric()) && !signal.is_match(token) {
            continue;
        }
        let lower = folded(token);
        match lower.as_str() {
            "c++" => {
                add(&mut aliases, "cplusplus");
                add(&mut aliases, "cpp");
            }
            "c#" => add(&mut aliases, "csharp"),
            "f#" => add(&mut aliases, "fsharp"),
            _ => {}
        }
        add(
            &mut aliases,
            &lower
                .chars()
                .filter(|c| c.is_alphanumeric())
                .collect::<String>(),
        );
        for component in delimiters.split(token).filter(|c| !c.is_empty()) {
            add(&mut aliases, component);
            for camel in parts(component, false) {
                for part in parts(camel, true) {
                    add(&mut aliases, part);
                }
            }
        }
        if aliases.len() >= 512 {
            break;
        }
    }
    aliases.join(" ")
}
pub fn redact(text: &str) -> String {
    static PATTERNS: OnceLock<Vec<Regex>> = OnceLock::new();
    let patterns=PATTERNS.get_or_init(||[
        r"(?s)-----BEGIN [A-Z ]*PRIVATE KEY-----.*?(?:-----END [A-Z ]*PRIVATE KEY-----|$)",
        r"(?i)\b(?:bearer\s+)[A-Za-z0-9._~+/=-]+", r"\b(?:AKIA|ASIA)[A-Z0-9]{16}\b",
        r"\b(?:sk-[A-Za-z0-9_-]{12,}|gh[pousr]_[A-Za-z0-9_]{16,}|xox[baprs]-[A-Za-z0-9-]+)\b",
        r"\beyJ[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+\b",
        r#"(?i)(?:api[_-]?key|access[_-]?token|secret|password)\s*[=:]\s*["']?[A-Za-z0-9_./+~-]{8,}"#,
    ].iter().map(|s|Regex::new(s).unwrap()).collect());
    patterns.iter().fold(text.to_owned(), |s, re| {
        re.replace_all(&s, "[REDACTED]").into_owned()
    })
}
/// A session title: the first non-blank line of `text`, at most `TITLE_CHARS`
/// characters. The store's `title_line` SQL macro is the same rule.
pub fn title_line(text: &str) -> String {
    let line = text.lines().map(str::trim).find(|l| !l.is_empty());
    bounded(line.unwrap_or(""), TITLE_CHARS, false)
}
pub fn bounded(text: &str, max: usize, do_redact: bool) -> String {
    let text = if do_redact {
        redact(text)
    } else {
        text.to_owned()
    };
    let mut out: String = text.chars().take(max).collect();
    if text.chars().count() > max && max > 0 {
        out.pop();
        out.push('…');
    }
    out
}
pub fn fixture_record(harness: Harness, session: &str, text: &str) -> Vec<Value> {
    match harness {
        Harness::Codex => vec![
            json!({"type":"session_meta","timestamp":"2026-10-01T00:00:00Z","payload":{"id":session,"cwd":"/projects/demo"}}),
            json!({"type":"response_item","timestamp":"2026-10-01T00:00:01Z","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":text}]}}),
        ],
        Harness::Claude => vec![
            json!({"type":"user","timestamp":"2026-10-01T00:00:01Z","sessionId":session,"cwd":"/projects/demo","message":{"content":[{"type":"text","text":text}]}}),
        ],
        Harness::Cursor => {
            vec![json!({"role":"user","message":{"content":[{"type":"text","text":text}]}})]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    #[test]
    fn titles_are_the_first_non_blank_line_bounded() {
        assert_eq!(
            title_line("\n  Fix the importer  \r\nlog"),
            "Fix the importer"
        );
        assert_eq!(title_line(" \n "), "");
        let long = title_line(&"é".repeat(200));
        assert_eq!(long.chars().count(), TITLE_CHARS);
        assert!(long.ends_with('…'));
    }
    #[test]
    fn capture_complete_lines_and_windows() -> Result<()> {
        let mut f = tempfile::NamedTempFile::new()?;
        f.write_all(b"{}\n[]\n{\"partial\":")?;
        let mut c = Capture::open(f.path(), 0, 0, 1024, Default::default())?;
        assert_eq!(c.next_record()?.unwrap().bytes, b"{}\n");
        assert_eq!(c.next_record()?.unwrap().bytes, b"[]\n");
        assert!(c.next_record()?.is_none());
        c.check_source()?;
        let (head, tail) = read_windows(f.path(), c.committed)?;
        assert_eq!(head, b"{}\n[]\n");
        assert_eq!(tail, head);
        assert_eq!(c.window_hashes(), window_hashes(&head, &tail));
        Ok(())
    }
    #[test]
    fn boundary_window_tracks_the_last_bytes_of_long_sources() -> Result<()> {
        let mut f = tempfile::NamedTempFile::new()?;
        let line = format!("{}\n", "x".repeat(1000));
        for _ in 0..200 {
            f.write_all(line.as_bytes())?;
        }
        let mut c = Capture::open(f.path(), 0, 0, 4096, Default::default())?;
        while c.next_record()?.is_some() {}
        let (head, tail) = read_windows(f.path(), c.committed)?;
        assert_eq!((head.len(), tail.len()), (WINDOW, WINDOW));
        assert_eq!(c.window_hashes(), window_hashes(&head, &tail));
        // Resuming from the stored windows yields the same windows as a full read.
        f.write_all(line.as_bytes())?;
        let mut resumed = Capture::open(f.path(), c.committed, c.ordinal, 4096, (head, tail))?;
        assert!(resumed.next_record()?.is_some());
        let (head, tail) = read_windows(f.path(), resumed.committed)?;
        assert_eq!(resumed.window_hashes(), window_hashes(&head, &tail));
        Ok(())
    }
    #[test]
    fn active_append_is_allowed() -> Result<()> {
        let mut f = tempfile::NamedTempFile::new()?;
        f.write_all(b"{}\n")?;
        let mut c = Capture::open(f.path(), 0, 0, 100, Default::default())?;
        assert!(c.next_record()?.is_some());
        f.write_all(b"{}\n")?;
        assert!(c.next_record()?.is_none());
        c.check_source()
    }
    #[test]
    fn oversized_record_is_bounded() -> Result<()> {
        let mut f = tempfile::NamedTempFile::new()?;
        f.write_all(&[b'x'; 200])?;
        assert!(
            Capture::open(f.path(), 0, 0, 100, Default::default())?
                .next_record()
                .is_err()
        );
        Ok(())
    }
    #[test]
    fn guardian_action_reads_every_codex_layout() {
        let block = |body: &str| {
            format!(
                "The Codex agent has requested the following action:\n\n\
                 >>> APPROVAL REQUEST START\n\n{body}\n\n>>> APPROVAL REQUEST END\n"
            )
        };
        let command = "Planned action JSON:\n\n{\n  \"tool\": \"exec_command\"\n}";
        let retry = format!("Retry reason:\n\nsandbox denied\n\n{command}");
        let network = "Below is a proposed network access request under review.\n\
                       Network access JSON:\n{\n  \"host\": \"example.com\"\n}";
        let quoted = "Planned action JSON:\n{\n  \"cmd\": \"rg '>>> APPROVAL REQUEST END\\n'\",\n  \
                      \"pattern\": \">>> APPROVAL REQUEST START\"\n}";
        let history = ">>> TRANSCRIPT START\n[1] user: hello\n>>> TRANSCRIPT END\n\n";
        for body in [command, retry.as_str(), network, quoted] {
            let sync = format!(
                "The following is the Codex agent history.\n{history}{}",
                block(body)
            );
            let delta = format!(
                ">>> TRANSCRIPT DELTA START\n>>> TRANSCRIPT DELTA END\n{}",
                block(body)
            );
            let asynchronous = format!("{history}{}", block(body));
            let nested = format!(
                "{history}[2] tool result:\n{}\n{}",
                block("Planned action JSON:\n{\"old\": true}"),
                block(body)
            );
            for prompt in [sync, delta, asynchronous, nested] {
                assert_eq!(guardian_action(&prompt), Some(body), "{prompt}");
            }
        }
        for unmarked in [
            "",
            history,
            ">>> APPROVAL REQUEST START\nno end",
            ">>> APPROVAL REQUEST END\n>>> APPROVAL REQUEST START\n",
            ">>> APPROVAL REQUEST START\n\n>>> APPROVAL REQUEST END\n",
            "said >>> APPROVAL REQUEST START\nx\nsaid >>> APPROVAL REQUEST END\n",
        ] {
            assert_eq!(guardian_action(unmarked), None, "{unmarked}");
        }
    }

    #[test]
    fn redact_before_limit() {
        assert!(!bounded("Bearer abcdefghijklmnopqrstuvwxyz", 12, true).contains("abcdef"));
    }
    #[test]
    fn timezone_and_dst() {
        assert_eq!(
            timestamp("2026-10-01", "America/Denver").unwrap(),
            "2026-10-01T06:00:00+00:00"
        );
        assert_eq!(
            timestamp("2026-11-01T01:30:00", "America/Denver").unwrap(),
            "2026-11-01T07:30:00+00:00"
        );
        assert!(timestamp("2026-10-27 09:30:00", "local").is_ok());
    }
}
