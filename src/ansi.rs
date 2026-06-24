//! Sanitize captured cell output for display in the TUI.
//!
//! ratatui renders bytes literally — it does **not** interpret ANSI — so raw
//! output from real commands (color, progress bars, cursor moves, OSC titles)
//! would corrupt the display. Per DESIGN §7 we sanitize at the TUI boundary only;
//! the CLI path passes raw bytes straight to the real terminal, which interprets
//! them itself.
//!
//! MVP policy is the **strip-everything** path: remove all escape sequences
//! (including SGR color), collapse carriage-return rewrites to their final
//! segment, expand tabs, and drop any stray control bytes. Parsing SGR into
//! ratatui styles (i.e. preserving color) is a deferred enhancement (DESIGN §7).

/// Tab stop width used when expanding `\t` to spaces.
const TAB_WIDTH: usize = 8;

/// Sanitize cell output for TUI display: strip ANSI escapes, normalize newlines,
/// collapse `\r` progress rewrites, expand tabs, and drop stray control chars.
///
/// Newlines are preserved (they're the line structure the renderer wraps on);
/// every other control character is removed or rendered harmless.
pub fn sanitize(input: &str) -> String {
    // Order matters: the escape stripper is vte-based and also consumes C0 controls
    // like `\r` and `\t`, so we must act on those *before* stripping — otherwise a
    // progress bar's `\r` rewrites get concatenated instead of collapsed.

    // 1. Normalize CRLF so a trailing `\r` isn't mistaken for a rewrite.
    let normalized = input.replace("\r\n", "\n");

    // 2. Per line: collapse `\r` rewrites to the final segment, then expand tabs to
    //    spaces (which survive stripping, unlike a literal `\t`).
    let mut pre = String::with_capacity(normalized.len());
    for (i, line) in normalized.split('\n').enumerate() {
        if i > 0 {
            pre.push('\n');
        }
        let line = line.rsplit('\r').next().unwrap_or(line);
        expand_tabs(&mut pre, line);
    }

    // 3. Strip ANSI/CSI/OSC escapes and any residual control bytes; printable text,
    //    the spaces we just emitted, and `\n` all survive.
    strip_ansi_escapes::strip_str(&pre)
}

/// Append `line` to `out`, expanding each `\t` to the next [`TAB_WIDTH`] column
/// stop. Other characters pass through untouched (escape stripping happens after).
fn expand_tabs(out: &mut String, line: &str) {
    let mut col = 0;
    for ch in line.chars() {
        if ch == '\t' {
            let spaces = TAB_WIDTH - (col % TAB_WIDTH);
            out.extend(std::iter::repeat_n(' ', spaces));
            col += spaces;
        } else {
            out.push(ch);
            col += 1; // char-count column; good enough for tab stops.
        }
    }
}

#[cfg(test)]
mod tests {
    use super::sanitize;

    #[test]
    fn strips_sgr_color_codes() {
        // Red "error", reset.
        let s = sanitize("\x1b[31merror\x1b[0m");
        assert_eq!(s, "error");
    }

    #[test]
    fn strips_cursor_moves_and_clears() {
        let s = sanitize("a\x1b[2J\x1b[Hb");
        assert_eq!(s, "ab");
    }

    #[test]
    fn strips_osc_title_sequence() {
        // OSC set-window-title, BEL-terminated, then real text.
        let s = sanitize("\x1b]0;my title\x07hello");
        assert_eq!(s, "hello");
    }

    #[test]
    fn collapses_carriage_return_progress() {
        let s = sanitize("10%\r50%\r100%\n");
        assert_eq!(s, "100%\n");
    }

    #[test]
    fn normalizes_crlf_without_eating_content() {
        let s = sanitize("line one\r\nline two\r\n");
        assert_eq!(s, "line one\nline two\n");
    }

    #[test]
    fn expands_tabs_to_stops() {
        // 'a' at col 0, tab → next stop at 8.
        assert_eq!(sanitize("a\tb"), "a       b");
        // A full-width tab from col 0.
        assert_eq!(sanitize("\tx"), "        x");
    }

    #[test]
    fn drops_stray_control_bytes() {
        // BEL and backspace are dropped; the visible text survives.
        let s = sanitize("foo\x07\x08bar");
        assert_eq!(s, "foobar");
    }

    #[test]
    fn leaves_plain_text_untouched() {
        let s = "just normal output\nwith two lines\n";
        assert_eq!(sanitize(s), s);
    }
}
