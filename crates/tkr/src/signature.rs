//! Shape signatures for emitted lines: collapse dynamic content (timestamps,
//! UUIDs, hex, digits) into placeholders so structurally identical lines hash
//! the same. Used by pattern-mining to surface candidate `suppress_regex` rules.

const MAX_SIG_LEN: usize = 80;

pub fn signature_of(line: &str) -> String {
    let s = replace_iso_timestamps(line);
    let s = replace_uuids(&s);
    let s = replace_long_hex(&s);
    let s = replace_ipv4(&s);
    let s = replace_digit_runs(&s);
    truncate_with_ellipsis(&s, MAX_SIG_LEN)
}

/// Convert a signature back into a candidate regex by escaping literal text and
/// expanding placeholders into character classes.
pub fn signature_to_regex(sig: &str) -> String {
    let mut out = String::with_capacity(sig.len() * 2);
    let mut chars = sig.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            'T' => out.push_str(r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}"),
            'U' => out.push_str(r"[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}"),
            'H' => out.push_str(r"[0-9a-f]{8,}"),
            'I' if chars.peek() == Some(&'P') => {
                chars.next();
                out.push_str(r"\d+\.\d+\.\d+\.\d+");
            }
            'N' => out.push_str(r"\d+"),
            _ => {
                if regex_meta(c) {
                    out.push('\\');
                }
                out.push(c);
            }
        }
    }
    out
}

/// Alias for `signature_to_regex`. Phase-1 noise ranker uses this name.
pub fn pattern_for_signature(sig: &str) -> String {
    signature_to_regex(sig)
}

fn regex_meta(c: char) -> bool {
    matches!(
        c,
        '.' | '+' | '*' | '?' | '(' | ')' | '[' | ']' | '{' | '}' | '|' | '^' | '$' | '\\' | '/'
    )
}

fn truncate_with_ellipsis(s: &str, max: usize) -> String {
    let total = s.chars().count();
    if total <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max).collect();
    out.push('…');
    out
}

fn replace_ipv4(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < bytes.len() {
        if let Some(n) = ipv4_len_at(&bytes[i..]) {
            out.push_str("IP");
            i += n;
        } else {
            // Safe because we only advance one byte when char_at is ASCII; otherwise
            // copy the full UTF-8 char.
            let c = s[i..].chars().next().unwrap();
            out.push(c);
            i += c.len_utf8();
        }
    }
    out
}

fn ipv4_len_at(bytes: &[u8]) -> Option<usize> {
    let mut consumed = 0usize;
    for octet in 0..4 {
        let mut digits = 0;
        while consumed + digits < bytes.len()
            && (bytes[consumed + digits] as char).is_ascii_digit()
            && digits < 3
        {
            digits += 1;
        }
        if digits == 0 {
            return None;
        }
        consumed += digits;
        if octet < 3 {
            if consumed >= bytes.len() || bytes[consumed] != b'.' {
                return None;
            }
            consumed += 1;
        }
    }
    if consumed < bytes.len() {
        let next = bytes[consumed] as char;
        if next.is_ascii_digit() || next == '.' {
            return None;
        }
    }
    Some(consumed)
}

fn replace_iso_timestamps(s: &str) -> String {
    rewrite_with(s, |rest| {
        let head: Vec<char> = rest.chars().take(19).collect();
        if head.len() == 19 && is_iso(&head) {
            Some((19, 'T'))
        } else {
            None
        }
    })
}

fn is_iso(c: &[char]) -> bool {
    let d = |i: usize| c[i].is_ascii_digit();
    d(0) && d(1)
        && d(2)
        && d(3)
        && c[4] == '-'
        && d(5)
        && d(6)
        && c[7] == '-'
        && d(8)
        && d(9)
        && c[10] == 'T'
        && d(11)
        && d(12)
        && c[13] == ':'
        && d(14)
        && d(15)
        && c[16] == ':'
        && d(17)
        && d(18)
}

fn replace_uuids(s: &str) -> String {
    rewrite_with(s, |rest| {
        let head: Vec<char> = rest.chars().take(36).collect();
        if head.len() == 36 && is_uuid(&head) {
            Some((36, 'U'))
        } else {
            None
        }
    })
}

fn is_uuid(c: &[char]) -> bool {
    let h = |i: usize| {
        let ch = c[i];
        ch.is_ascii_digit() || ('a'..='f').contains(&ch) || ('A'..='F').contains(&ch)
    };
    if !(c[8] == '-' && c[13] == '-' && c[18] == '-' && c[23] == '-') {
        return false;
    }
    for i in [
        0, 1, 2, 3, 4, 5, 6, 7, 9, 10, 11, 12, 14, 15, 16, 17, 19, 20, 21, 22, 24, 25, 26, 27, 28,
        29, 30, 31, 32, 33, 34, 35,
    ] {
        if !h(i) {
            return false;
        }
    }
    true
}

fn replace_long_hex(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut run = String::new();
    for c in s.chars() {
        if c.is_ascii_digit() || ('a'..='f').contains(&c) {
            run.push(c);
        } else {
            flush_hex(&mut out, &run);
            run.clear();
            out.push(c);
        }
    }
    flush_hex(&mut out, &run);
    out
}

fn flush_hex(out: &mut String, run: &str) {
    if run.chars().count() >= 8 && run.chars().any(|c| c.is_ascii_alphabetic()) {
        out.push('H');
    } else {
        out.push_str(run);
    }
}

fn replace_digit_runs(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_digits = false;
    for c in s.chars() {
        if c.is_ascii_digit() {
            if !in_digits {
                out.push('N');
                in_digits = true;
            }
        } else {
            in_digits = false;
            out.push(c);
        }
    }
    out
}

/// Walk `s` char by char. At each position, `f` may opt to consume N chars and
/// emit a single placeholder char; otherwise the current char is copied.
fn rewrite_with<F>(s: &str, mut f: F) -> String
where
    F: FnMut(&str) -> Option<(usize, char)>,
{
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while !rest.is_empty() {
        if let Some((take, ch)) = f(rest) {
            out.push(ch);
            let byte_offset = rest
                .char_indices()
                .nth(take)
                .map(|(b, _)| b)
                .unwrap_or(rest.len());
            rest = &rest[byte_offset..];
        } else {
            let c = rest.chars().next().unwrap();
            out.push(c);
            rest = &rest[c.len_utf8()..];
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iso_timestamp_collapses() {
        assert_eq!(signature_of("[2026-04-28T21:00:00] hello"), "[T] hello");
    }

    #[test]
    fn iso_then_short_hex_just_collapses_digits() {
        // `abc123` is only 6 hex chars — too short for H; trailing digits → N.
        assert_eq!(
            signature_of("[2026-04-28T21:00:00] req=abc123"),
            "[T] req=abcN"
        );
    }

    #[test]
    fn digit_run_collapses() {
        assert_eq!(signature_of("user 12345 logged in"), "user N logged in");
    }

    #[test]
    fn long_hex_collapses() {
        assert_eq!(
            signature_of("hash: 81cda431a68d54a55707179f546a4b6c449e92e2"),
            "hash: H"
        );
    }

    #[test]
    fn plain_text_unchanged() {
        assert_eq!(signature_of("plain text"), "plain text");
    }

    #[test]
    fn uuid_collapses() {
        assert_eq!(
            signature_of("id=550e8400-e29b-41d4-a716-446655440000 ok"),
            "id=U ok"
        );
    }

    #[test]
    fn truncates_to_80_chars_with_ellipsis() {
        let long = "x".repeat(200);
        let sig = signature_of(&long);
        assert_eq!(sig.chars().count(), 81);
        assert!(sig.ends_with('…'));
    }

    #[test]
    fn ipv4_collapses() {
        assert_eq!(
            signature_of("client 10.0.0.1 connected"),
            "client IP connected"
        );
    }

    #[test]
    fn ipv4_with_port_keeps_port_as_n() {
        assert_eq!(signature_of("listen 127.0.0.1:8080"), "listen IP:N");
    }

    #[test]
    fn pattern_for_signature_round_trip_matches_original() {
        let line = "[2026-04-28T21:00:00] req from 10.0.0.5 took 42ms";
        let sig = signature_of(line);
        let pat = pattern_for_signature(&sig);
        let re = regex::Regex::new(&pat).expect("regex compiles");
        assert!(re.is_match(line), "pattern {pat:?} should match {line:?}");
    }

    #[test]
    fn pattern_for_signature_handles_ip() {
        assert_eq!(
            pattern_for_signature("from IP done"),
            r"from \d+\.\d+\.\d+\.\d+ done"
        );
    }

    #[test]
    fn short_hex_left_alone() {
        assert_eq!(signature_of("abc1234"), "abcN");
    }

    #[test]
    fn pure_digit_long_run_is_just_n() {
        // 12+ digits — not "hex with letters", so it goes the digit path.
        assert_eq!(signature_of("123456789012"), "N");
    }

    #[test]
    fn signature_to_regex_basic() {
        assert_eq!(
            signature_to_regex("user N logged in"),
            r"user \d+ logged in"
        );
    }

    #[test]
    fn signature_to_regex_escapes_meta() {
        assert_eq!(signature_to_regex("(N)"), r"\(\d+\)");
    }

    #[test]
    fn utf8_safe() {
        assert_eq!(signature_of("├── anyhow v1.0.102"), "├── anyhow vN.N.N");
    }
}
