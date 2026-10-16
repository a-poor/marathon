#[path = "support/output_properties.rs"]
mod properties;

#[test]
fn arbitrary_bytes_and_terminal_sequences_are_chunk_invariant() {
    // Fixed seed keeps the ordinary test suite reproducible. cargo-fuzz explores
    // and minimizes further cases using the same properties.
    let mut seed = 0x4d41524154484f4eu64;
    let mut next = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let tokens: &[&[u8]] = &[
        b"\r",
        b"\r\n",
        b"\t",
        b"\x1b[31m",
        b"\x1b[0m",
        b"\x1b]title\x07",
        b"\x1bPunterminated",
        b"\x1b\\",
        b"\x18",
        b"\xf0\x9f",
        b"\xff",
        "café 界 👩‍💻\n".as_bytes(),
    ];
    properties::check_output(&[]);
    for _ in 0..128 {
        let mut input = Vec::new();
        for _ in 0..(next() % 128) {
            let value = next();
            if value % 2 == 0 {
                input.push((value >> 8) as u8);
            } else {
                input.extend_from_slice(tokens[(value as usize >> 8) % tokens.len()]);
            }
        }
        properties::check_output(&input);
    }
}

#[test]
fn output_pages_reconstruct_multibyte_text_across_boundaries() {
    let page_bytes = marathon::output::PAGE_BYTES as usize;
    for character in ['é', '界', '🦀'] {
        for split in 1..character.len_utf8() {
            let mut input = "x".repeat(page_bytes - split);
            input.push(character);
            input.push_str(&"café 界 🦀\n".repeat(page_bytes / 8));
            properties::check_output(input.as_bytes());
        }
    }
}
