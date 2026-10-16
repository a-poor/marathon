use std::io::Read;

use marathon::output::{OutputCapture, PAGE_BYTES};

/// Shared by deterministic tests and the coverage-guided fuzzer. Never runs commands.
pub fn check_output(bytes: &[u8]) {
    let whole = OutputCapture::default();
    whole.append(bytes).unwrap();
    whole.finish().unwrap();
    let expected = whole.text().unwrap();
    assert!(expected.bytes().all(|b| b >= 0x20 || b == b'\n'));

    for bytewise in [true, false] {
        let capture = OutputCapture::default();
        let mut offset = 0;
        // Empty chunks must be harmless, including with a pending UTF-8 sequence.
        capture.append(&[]).unwrap();
        while offset < bytes.len() {
            let size = if bytewise {
                1
            } else {
                1 + usize::from(bytes[offset]) % 31
            };
            let end = (offset + size).min(bytes.len());
            capture.append(&bytes[offset..end]).unwrap();
            capture.append(&[]).unwrap();
            offset = end;
        }
        capture.finish().unwrap();
        assert_eq!(capture.text().unwrap(), expected, "bytewise={bytewise}");

        let mut raw = Vec::new();
        capture
            .raw_reader()
            .unwrap()
            .unwrap()
            .read_to_end(&mut raw)
            .unwrap();
        assert_eq!(raw, bytes);

        let mut pages = String::new();
        let total = expected.len() as u64;
        for page in 0..total.div_ceil(PAGE_BYTES).max(1) {
            let window = capture.window(Some(page), true).unwrap();
            assert_eq!(window.start, pages.len() as u64);
            assert_eq!(window.end, window.start + window.text.len() as u64);
            assert_eq!(window.total, total);
            assert!(window.text.len() as u64 <= PAGE_BYTES + 3);
            pages.push_str(&window.text);
        }
        assert_eq!(pages, expected);
        let tail = capture.window(None, false).unwrap();
        assert!(expected.ends_with(&tail.text));
        assert!(tail.text.len() as u64 <= PAGE_BYTES);
    }
}
