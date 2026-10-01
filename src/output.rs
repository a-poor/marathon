//! Per-run disk captures. Only a bounded display window is read back into memory.

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::sync::{Arc, Mutex};

use tempfile::NamedTempFile;

/// Bounds both the live tail and each expanded page (including very long lines).
pub const PAGE_BYTES: u64 = 16 * 1024;
pub const TAIL_LINES: usize = 25;

/// Clones share one run's capture; replacing/dropping the last owner deletes it.
/// Capture files are independent of the runbook's user-owned scratch directory.
#[derive(Clone, Debug, Default)]
pub struct OutputCapture(Arc<Mutex<Option<Store>>>);

#[derive(Debug)]
struct Store {
    raw: NamedTempFile,
    clean: NamedTempFile,
    decoder: Decoder,
    len: u64,
    line_start: u64,
    failed: bool,
}

#[derive(Debug, Default)]
struct Decoder {
    escape: Escape,
    utf8: Vec<u8>, // at most one incomplete UTF-8 character
    cr: bool,
    column: usize, // modulo the tab width
}

#[derive(Debug, Default, Clone, Copy)]
enum Escape {
    #[default]
    Text,
    Esc,
    Intermediate,
    Csi,
    String {
        osc: bool,
        esc: bool,
    },
}

#[derive(Debug)]
pub struct OutputWindow {
    pub text: String,
    pub start: u64,
    pub end: u64,
    pub total: u64,
}

impl OutputCapture {
    pub fn create() -> io::Result<Self> {
        let capture = Self::default();
        capture.with_store(|_| Ok(()))?;
        Ok(capture)
    }

    fn with_store<T>(&self, f: impl FnOnce(&mut Store) -> io::Result<T>) -> io::Result<T> {
        let mut guard = self
            .0
            .lock()
            .map_err(|_| io::Error::other("capture lock poisoned"))?;
        if guard.is_none() {
            *guard = Some(Store {
                raw: tempfile::Builder::new()
                    .prefix("marathon-output-")
                    .tempfile()?,
                clean: tempfile::Builder::new()
                    .prefix("marathon-display-")
                    .tempfile()?,
                decoder: Decoder::default(),
                len: 0,
                line_start: 0,
                failed: false,
            });
        }
        f(guard.as_mut().unwrap())
    }

    /// Called by the runner on a blocking worker, never from the draw loop.
    pub fn append(&self, bytes: &[u8]) -> io::Result<()> {
        self.with_store(|store| {
            if store.failed {
                return Err(io::Error::other("output capture is incomplete"));
            }
            let result = store.append(bytes);
            store.failed |= result.is_err();
            result
        })
    }

    pub fn finish(&self) -> io::Result<()> {
        self.with_store(|store| {
            if store.failed {
                return Err(io::Error::other("output capture is incomplete"));
            }
            let result = (|| {
                if store.decoder.cr {
                    store.clean.as_file_mut().set_len(store.line_start)?;
                    store
                        .clean
                        .as_file_mut()
                        .seek(SeekFrom::Start(store.line_start))?;
                    store.len = store.line_start;
                    store.decoder.cr = false;
                }
                if !store.decoder.utf8.is_empty() {
                    store.decoder.utf8.clear();
                    store.clean.as_file_mut().write_all("�".as_bytes())?;
                    store.len += 3;
                }
                Ok(())
            })();
            store.failed |= result.is_err();
            result
        })
    }

    pub fn is_empty(&self) -> bool {
        self.0.lock().unwrap().as_ref().is_none_or(|s| s.len == 0)
    }

    /// Read a bounded cleaned page, or the last 25 lines when collapsed. Offsets
    /// address UTF-8 bytes on disk, not an in-memory index that grows with output.
    pub fn window(&self, page: Option<u64>, expanded: bool) -> io::Result<OutputWindow> {
        let guard = self
            .0
            .lock()
            .map_err(|_| io::Error::other("capture lock poisoned"))?;
        let Some(store) = guard.as_ref() else {
            return Ok(OutputWindow {
                text: String::new(),
                start: 0,
                end: 0,
                total: 0,
            });
        };
        let total = store.len;
        let last_page = total.saturating_sub(1) / PAGE_BYTES;
        let start = if expanded {
            page.unwrap_or(last_page).min(last_page) * PAGE_BYTES
        } else {
            total.saturating_sub(PAGE_BYTES)
        };
        // Assign a character crossing a page boundary to the later page. Looking
        // back at most three bytes avoids an empty last page of continuation bytes.
        let read_start = if expanded {
            start.saturating_sub(3)
        } else {
            start
        };
        let mut file = store.clean.reopen()?;
        file.seek(SeekFrom::Start(read_start))?;
        let limit = (start + PAGE_BYTES).min(total) - read_start;
        let mut bytes = vec![0; limit as usize];
        file.read_exact(&mut bytes)?;
        let mut first = (start - read_start) as usize;
        if expanded {
            while first > 0 && first < bytes.len() && bytes[first] & 0xc0 == 0x80 {
                first -= 1;
            }
        } else {
            while first < bytes.len() && bytes[first] & 0xc0 == 0x80 {
                first += 1;
            }
        }
        let text = match std::str::from_utf8(&bytes[first..]) {
            Ok(text) => text,
            Err(e) if e.error_len().is_none() && read_start + (bytes.len() as u64) < total => {
                std::str::from_utf8(&bytes[first..first + e.valid_up_to()]).unwrap()
            }
            Err(e) => return Err(io::Error::new(io::ErrorKind::InvalidData, e)),
        };
        let mut text = text.to_owned();
        let mut start = read_start + first as u64;
        let end = start + text.len() as u64;
        if !expanded {
            let trim = text.strip_suffix('\n').unwrap_or(&text);
            if let Some((at, _)) = trim.rmatch_indices('\n').nth(TAIL_LINES - 1) {
                start += (at + 1) as u64;
                text.drain(..at + 1);
            }
        }
        Ok(OutputWindow {
            text,
            start,
            end,
            total,
        })
    }

    /// Explicit copying is the only operation that allocates the full cleaned
    /// output. No raw buffer or second sanitization pass is needed.
    pub fn text(&self) -> io::Result<String> {
        let guard = self
            .0
            .lock()
            .map_err(|_| io::Error::other("capture lock poisoned"))?;
        let Some(store) = guard.as_ref() else {
            return Ok(String::new());
        };
        if store.failed {
            return Err(io::Error::other("output capture is incomplete"));
        }
        let mut text = String::new();
        store
            .clean
            .reopen()?
            .take(store.len)
            .read_to_string(&mut text)?;
        if text.len() as u64 != store.len {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "output spool was truncated",
            ));
        }
        Ok(text)
    }

    /// Open the original byte capture without decoding or sanitization.
    pub fn raw_reader(&self) -> io::Result<Option<File>> {
        let guard = self
            .0
            .lock()
            .map_err(|_| io::Error::other("capture lock poisoned"))?;
        guard.as_ref().map(|s| s.raw.reopen()).transpose()
    }

    #[cfg(test)]
    pub(crate) fn paths(&self) -> Vec<std::path::PathBuf> {
        self.0
            .lock()
            .unwrap()
            .as_ref()
            .map(|s| vec![s.raw.path().to_owned(), s.clean.path().to_owned()])
            .unwrap_or_default()
    }

    #[cfg(test)]
    pub(crate) fn fail_writes(&self) {
        let mut guard = self.0.lock().unwrap();
        let store = guard.as_mut().unwrap();
        // A real write error, without global environment/permission changes.
        *store.raw.as_file_mut() = File::open(store.raw.path()).unwrap();
    }
}

impl Store {
    fn append(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.raw.write_all(bytes)?;
        // Bound transient work even for callers passing arbitrarily large chunks.
        for bytes in bytes.chunks(8192) {
            let mut pending = Vec::with_capacity(bytes.len());
            for &byte in bytes {
                if self.decoder.cr {
                    self.decoder.cr = false;
                    if byte != b'\n' {
                        self.flush_clean(&mut pending)?;
                        self.clean.as_file_mut().set_len(self.line_start)?;
                        self.clean
                            .as_file_mut()
                            .seek(SeekFrom::Start(self.line_start))?;
                        self.len = self.line_start;
                        self.decoder.column = 0;
                    }
                }
                self.decoder.feed(byte, &mut pending);
                if pending.last() == Some(&b'\n') {
                    self.line_start = self.len + pending.len() as u64;
                }
            }
            self.flush_clean(&mut pending)?;
        }
        Ok(())
    }

    fn flush_clean(&mut self, pending: &mut Vec<u8>) -> io::Result<()> {
        self.clean.write_all(pending)?;
        self.len += pending.len() as u64;
        pending.clear();
        Ok(())
    }
}

impl Decoder {
    // This escape filter intentionally never accumulates CSI/OSC/DCS payloads:
    // even an unterminated multi-gigabyte escape sequence has constant state.
    fn feed(&mut self, byte: u8, out: &mut Vec<u8>) {
        if !self.utf8.is_empty() {
            self.utf8.push(byte);
            match std::str::from_utf8(&self.utf8) {
                Ok(_) => {
                    out.extend_from_slice(&self.utf8);
                    self.utf8.clear();
                    self.column = (self.column + 1) % 8;
                    return;
                }
                Err(e) if e.error_len().is_none() => return,
                Err(e) => {
                    let invalid = e.error_len().unwrap();
                    let remaining = self.utf8.split_off(invalid);
                    self.utf8.clear();
                    out.extend_from_slice("�".as_bytes());
                    self.column = (self.column + 1) % 8;
                    for b in remaining {
                        self.feed(b, out);
                    }
                    return;
                }
            }
        }
        match self.escape {
            Escape::String { osc, esc } => {
                self.escape = if (osc && byte == 7)
                    || (esc && byte == b'\\')
                    || matches!(byte, 0x18 | 0x1a)
                {
                    Escape::Text
                } else {
                    Escape::String {
                        osc,
                        esc: byte == 0x1b,
                    }
                };
            }
            _ if byte == 0x1b => self.escape = Escape::Esc,
            _ if matches!(byte, 0x18 | 0x1a) => self.escape = Escape::Text,
            Escape::Esc => {
                self.escape = match byte {
                    b'[' => Escape::Csi,
                    b']' => Escape::String {
                        osc: true,
                        esc: false,
                    },
                    b'P' | b'X' | b'^' | b'_' => Escape::String {
                        osc: false,
                        esc: false,
                    },
                    0x20..=0x2f => Escape::Intermediate,
                    _ => Escape::Text,
                }
            }
            Escape::Intermediate => {
                if (0x30..=0x7e).contains(&byte) {
                    self.escape = Escape::Text;
                }
            }
            Escape::Csi => {
                if (0x40..=0x7e).contains(&byte) {
                    self.escape = Escape::Text;
                }
            }
            Escape::Text => match byte {
                b'\r' => self.cr = true,
                b'\n' => {
                    out.push(byte);
                    self.column = 0;
                }
                b'\t' => {
                    out.extend(std::iter::repeat_n(b' ', 8 - self.column));
                    self.column = 0;
                }
                0x00..=0x1f | 0x7f => {}
                0xc2..=0xf4 => self.utf8.push(byte),
                0x80..=0xff => {
                    out.extend_from_slice("�".as_bytes());
                    self.column = (self.column + 1) % 8;
                }
                _ => {
                    out.push(byte);
                    self.column = (self.column + 1) % 8;
                }
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunk_boundaries_preserve_utf8_escapes_crlf_and_tabs() {
        let raw = b"old\rnew\r\n\x1b[31mred\x1b[0m\t\xe2\x82\xac\xff\n\x1b]0;hidden\x1b\\visible\x07\x08\npartial\xf0\x9f";
        for size in 1..=raw.len() {
            let capture = OutputCapture::default();
            for chunk in raw.chunks(size) {
                capture.append(chunk).unwrap();
            }
            capture.finish().unwrap();
            assert_eq!(
                capture.text().unwrap(),
                "new\nred     €�\nvisible\npartial�",
                "chunk size {size}"
            );
            let mut saved = Vec::new();
            capture
                .raw_reader()
                .unwrap()
                .unwrap()
                .read_to_end(&mut saved)
                .unwrap();
            assert_eq!(saved, raw);
        }
    }

    #[test]
    fn partial_output_and_progress_are_visible_before_completion() {
        let capture = OutputCapture::default();
        capture.append(b"ready").unwrap();
        assert_eq!(capture.window(None, false).unwrap().text, "ready");
        capture.append(b"\r").unwrap();
        capture.append(b"\nold\rnew\t!").unwrap();
        assert_eq!(capture.text().unwrap(), "ready\nnew     !");
        capture.append(b"\r").unwrap();
        capture.finish().unwrap();
        assert_eq!(capture.text().unwrap(), "ready\n");
    }

    #[test]
    fn large_capture_has_bounded_windows_and_complete_copy_and_raw_bytes() {
        let capture = OutputCapture::default();
        let chunk = "hello €\n".repeat(1024);
        for _ in 0..256 {
            capture.append(chunk.as_bytes()).unwrap();
        }
        let window = capture.window(None, false).unwrap();
        assert!(window.text.len() <= PAGE_BYTES as usize + 3);
        assert_eq!(window.text.lines().count(), TAIL_LINES);
        let text = capture.text().unwrap();
        assert_eq!(text.len(), chunk.len() * 256);
        assert_eq!(
            capture
                .raw_reader()
                .unwrap()
                .unwrap()
                .metadata()
                .unwrap()
                .len(),
            text.len() as u64
        );
        let mut pages = String::new();
        for page in 0..window.total.div_ceil(PAGE_BYTES) {
            let window = capture.window(Some(page), true).unwrap();
            assert!(window.text.len() <= PAGE_BYTES as usize + 3);
            pages.push_str(&window.text);
        }
        assert_eq!(pages, text);
        assert!(
            capture
                .0
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .decoder
                .utf8
                .capacity()
                <= 8
        );
    }

    #[test]
    fn giant_lines_and_unterminated_escapes_do_not_accumulate_in_memory() {
        let capture = OutputCapture::default();
        let chunk = vec![b'x'; 8192];
        for _ in 0..256 {
            capture.append(&chunk).unwrap();
        }
        assert_eq!(
            capture.window(None, false).unwrap().text.len(),
            PAGE_BYTES as usize
        );
        capture.append(b"\rshort\n\x1b]0;").unwrap();
        for _ in 0..256 {
            capture.append(&chunk).unwrap();
        }
        assert_eq!(capture.text().unwrap(), "short\n");
        capture.append(b"\x07done").unwrap();
        assert_eq!(capture.text().unwrap(), "short\ndone");
    }

    #[test]
    fn tail_counts_blank_lines_and_pages_preserve_split_characters() {
        let capture = OutputCapture::default();
        capture
            .append(format!("head{}", "\n".repeat(100)).as_bytes())
            .unwrap();
        assert_eq!(capture.window(None, false).unwrap().text, "\n".repeat(25));
        let capture = OutputCapture::default();
        let text = format!("{}€🦀end", "x".repeat(PAGE_BYTES as usize - 1));
        capture.append(text.as_bytes()).unwrap();
        let first = capture.window(Some(0), true).unwrap();
        let second = capture.window(Some(1), true).unwrap();
        assert_eq!(first.text + &second.text, text);
    }

    #[test]
    fn final_character_crossing_page_boundary_is_visible_on_latest_page() {
        for ch in ["é", "€", "🦀"] {
            for split in 1..ch.len() {
                let capture = OutputCapture::default();
                let text = format!("{}{ch}", "x".repeat(PAGE_BYTES as usize - split));
                capture.append(text.as_bytes()).unwrap();
                let latest = capture.window(None, true).unwrap();
                assert_eq!(latest.text, ch);
                assert_eq!(
                    capture.window(Some(0), true).unwrap().text + &latest.text,
                    text
                );
            }
        }
    }

    #[test]
    fn dropping_last_owner_removes_files_and_read_errors_are_reported() {
        let capture = OutputCapture::create().unwrap();
        capture.append(b"hello").unwrap();
        let paths = capture.paths();
        let owner = capture.clone();
        drop(capture);
        assert!(paths.iter().all(|p| p.exists()));
        std::fs::OpenOptions::new()
            .write(true)
            .open(&paths[1])
            .unwrap()
            .set_len(0)
            .unwrap();
        assert!(owner.window(None, true).is_err());
        assert!(owner.text().is_err());
        std::fs::remove_file(&paths[1]).unwrap();
        assert!(owner.window(None, false).is_err());
        assert!(owner.text().is_err());
        drop(owner);
        assert!(paths.iter().all(|p| !p.exists()));
    }

    #[test]
    fn display_spool_failures_also_invalidate_copy_and_completion() {
        for finalize in [false, true] {
            let capture = OutputCapture::create().unwrap();
            capture.append(b"prefix\xe2").unwrap();
            {
                let mut guard = capture.0.lock().unwrap();
                let store = guard.as_mut().unwrap();
                *store.clean.as_file_mut() = File::open(store.clean.path()).unwrap();
            }
            if finalize {
                assert!(capture.finish().is_err());
            } else {
                assert!(capture.append(b"\x82\xac").is_err());
            }
            assert!(capture.text().is_err());
        }
    }

    #[test]
    fn failed_writes_never_report_a_complete_copy() {
        let capture = OutputCapture::create().unwrap();
        capture.append(b"prefix").unwrap();
        capture.fail_writes();
        assert!(capture.append(b"lost").is_err());
        assert!(capture.text().is_err());
        assert!(capture.finish().is_err());
        assert_eq!(capture.window(None, false).unwrap().text, "prefix");
    }
}
