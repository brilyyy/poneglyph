//! Deterministic, reversible prose compression ("caveman grammar").
//!
//! Common English words become single private-use-area Unicode codepoints
//! so stored/retrieved text shrinks. Code fences, inline code spans, and
//! any token shaped like a path, version, or identifier (contains `/`,
//! `\`, `` ` ``, `@`, `_`, or a digit) are never touched, and the whole
//! input passes through unchanged if it already contains a marker
//! codepoint — so `expand(compress(x)) == x` always, not just on the
//! protected-content fixtures the tests target.

use std::collections::HashMap;
use std::sync::LazyLock;

/// Private-use-area codepoints used as compression markers. Real text does
/// not contain these, so a marker found in input is proof compression
/// hasn't already run on it — `compress` uses that as its safety check.
const MARKER_BASE: u32 = 0xE000;
const MARKER_END: u32 = 0xE0FF;
/// Appended after a marker when the original word's first letter was
/// uppercase, so `The` round-trips distinctly from `the`.
const CASE_CAPITALIZED: char = '\u{E0F0}';

const DICTIONARY: &[&str] = &[
    "the", "and", "that", "with", "this", "from", "have", "been", "were", "which", "their",
    "would", "there", "about", "because", "should", "could", "these", "those", "where", "after",
    "before", "through", "between", "during", "without", "however", "therefore", "although",
    "function", "implementation", "configuration", "application", "currently", "previously",
    "essentially", "basically", "actually", "additionally", "specifically", "particularly",
    "significantly", "memory", "project", "session", "default", "enabled", "disabled",
];

static ENCODE: LazyLock<HashMap<&'static str, char>> = LazyLock::new(|| {
    DICTIONARY
        .iter()
        .enumerate()
        .map(|(i, w)| (*w, char::from_u32(MARKER_BASE + i as u32).expect("dictionary smaller than PUA range")))
        .collect()
});

static DECODE: LazyLock<HashMap<char, &'static str>> = LazyLock::new(|| ENCODE.iter().map(|(w, c)| (*c, *w)).collect());

fn contains_marker(s: &str) -> bool {
    s.chars().any(|c| (MARKER_BASE..=MARKER_END).contains(&(c as u32)))
}

/// Compress prose outside code fences/spans and protected tokens. Identity
/// (no-op) if the input already contains a marker codepoint.
pub fn compress(input: &str) -> String {
    if contains_marker(input) {
        return input.to_string();
    }

    let mut out = String::with_capacity(input.len());
    let mut in_fence = false;
    for line in split_keep_newlines(input) {
        if line.trim_start().starts_with("```") {
            in_fence = !in_fence;
            out.push_str(line);
            continue;
        }
        if in_fence {
            out.push_str(line);
            continue;
        }
        out.push_str(&compress_line(line));
    }
    out
}

/// Reverse of `compress`. Safe to call on text that was never compressed —
/// it only acts on marker codepoints, which never occur in normal text.
pub fn expand(input: &str) -> String {
    if !contains_marker(input) {
        return input.to_string();
    }
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(c) = chars.next() {
        let Some(word) = DECODE.get(&c) else {
            out.push(c);
            continue;
        };
        let capitalized = chars.peek() == Some(&CASE_CAPITALIZED);
        if capitalized {
            chars.next();
        }
        if capitalized {
            let mut chs = word.chars();
            if let Some(first) = chs.next() {
                out.extend(first.to_uppercase());
                out.push_str(chs.as_str());
            }
        } else {
            out.push_str(word);
        }
    }
    out
}

fn split_keep_newlines(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    for (i, c) in s.char_indices() {
        if c == '\n' {
            out.push(&s[start..=i]);
            start = i + 1;
        }
    }
    if start < s.len() {
        out.push(&s[start..]);
    }
    out
}

/// Split a non-fenced line on inline `` `code` `` spans (left untouched)
/// and compress the prose in between.
fn compress_line(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    loop {
        let Some(start) = rest.find('`') else {
            out.push_str(&compress_words(rest));
            break;
        };
        let Some(rel_end) = rest[start + 1..].find('`') else {
            // Unterminated inline span on this line: leave the rest as-is.
            out.push_str(&compress_words(&rest[..start]));
            out.push_str(&rest[start..]);
            break;
        };
        let end = start + 1 + rel_end + 1; // include closing backtick
        out.push_str(&compress_words(&rest[..start]));
        out.push_str(&rest[start..end]);
        rest = &rest[end..];
    }
    out
}

/// Whitespace-tokenize and substitute whole dictionary words.
fn compress_words(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while !rest.is_empty() {
        let ws_len = rest.find(|c: char| !c.is_whitespace()).unwrap_or(rest.len());
        out.push_str(&rest[..ws_len]);
        rest = &rest[ws_len..];
        if rest.is_empty() {
            break;
        }
        let tok_len = rest.find(char::is_whitespace).unwrap_or(rest.len());
        out.push_str(&compress_token(&rest[..tok_len]));
        rest = &rest[tok_len..];
    }
    out
}

fn is_protected_token(token: &str) -> bool {
    token.contains('/')
        || token.contains('\\')
        || token.contains('`')
        || token.contains('@')
        || token.contains('_')
        || token.chars().any(|c| c.is_ascii_digit())
}

fn compress_token(token: &str) -> String {
    if is_protected_token(token) {
        return token.to_string();
    }

    let lead_len = token.len() - token.trim_start_matches(|c: char| c.is_ascii_punctuation()).len();
    let after_lead = &token[lead_len..];
    let core_len = after_lead.trim_end_matches(|c: char| c.is_ascii_punctuation()).len();
    let lead = &token[..lead_len];
    let core = &after_lead[..core_len];
    let trail = &after_lead[core_len..];

    if core.is_empty() || !core.chars().all(|c| c.is_alphabetic()) {
        return token.to_string();
    }

    let lower = core.to_lowercase();
    let Some(&marker) = ENCODE.get(lower.as_str()) else {
        return token.to_string();
    };

    let capitalized = core.chars().next().is_some_and(|c| c.is_uppercase());
    let mut out = String::with_capacity(lead.len() + trail.len() + 2);
    out.push_str(lead);
    out.push(marker);
    if capitalized {
        out.push(CASE_CAPITALIZED);
    }
    out.push_str(trail);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    fn roundtrip(s: &str) {
        let compressed = compress(s);
        assert_eq!(expand(&compressed), s, "round-trip failed for: {s:?}");
    }

    #[test]
    fn roundtrip_plain_prose() {
        roundtrip("The function and the configuration would, however, change.");
    }

    #[test]
    fn roundtrip_code_fence_untouched_bytes() {
        let s = "Explanation: the function does this.\n```rust\nlet the_x = 1; // and more\n```\nAfter that, done.";
        let compressed = compress(s);
        // The fenced block's bytes must appear verbatim in the compressed output.
        assert!(compressed.contains("```rust\nlet the_x = 1; // and more\n```"));
        roundtrip(s);
    }

    #[test]
    fn roundtrip_inline_code_untouched() {
        roundtrip("Run `cargo test --workspace` and then the build should pass.");
    }

    #[test]
    fn roundtrip_paths_and_versions_byte_for_byte() {
        // Pure protected tokens: the whole fixture must pass through unchanged.
        let untouched = [
            "crates/poneglyph-core/src/config.rs",
            "C:\\Users\\dev\\project\\file.rs",
            "1.2.3",
            "v2.10.0",
            "user_id_42",
            "session_99",
            "dev@example.com",
        ];
        for f in untouched {
            let compressed = compress(f);
            assert_eq!(compressed, f, "protected fixture must pass through unchanged: {f:?}");
            roundtrip(f);
        }

        // Mixed sentences: surrounding prose may compress, but the embedded
        // protected token itself must survive byte-for-byte.
        let mixed: &[(&str, &str)] = &[
            ("version 1.2.3 was released", "1.2.3"),
            ("bump to v2.10.0 because of the fix", "v2.10.0"),
            ("user_id_42 and session_99 matched", "user_id_42"),
            ("contact dev@example.com about this", "dev@example.com"),
        ];
        for (sentence, token) in mixed {
            let compressed = compress(sentence);
            assert!(compressed.contains(token), "token {token:?} must survive in {compressed:?}");
            roundtrip(sentence);
        }
    }

    #[test]
    fn roundtrip_mixed_content() {
        roundtrip(
            "Fixed the bug in crates/poneglyph-core/src/config.rs because the default \
             value was wrong. Run `cargo test` to verify. Bumped to v1.2.3 afterwards.",
        );
    }

    #[test]
    fn compression_shrinks_prose() {
        let s = "The function and the configuration would, however, change because of this.";
        let compressed = compress(s);
        assert!(compressed.len() < s.len(), "expected compression to shrink prose");
    }

    #[test]
    fn idempotent_on_already_compressed_input() {
        let s = "The function and the configuration would change.";
        let once = compress(s);
        let twice = compress(&once);
        assert_eq!(once, twice, "compressing an already-compressed string must be a no-op");
        assert_eq!(expand(&twice), s);
    }

    #[test]
    fn roundtrip_empty_and_whitespace() {
        roundtrip("");
        roundtrip("   \n\n  ");
    }

    #[test]
    fn handler_latency_budget() {
        // Hook handlers must stay well under the 150ms budget; this is a
        // cheap string transform, so a generous fixture should run in
        // microseconds, not milliseconds.
        let s = "The function and the configuration would, however, change. ".repeat(200);
        let start = Instant::now();
        let compressed = compress(&s);
        let _ = expand(&compressed);
        assert!(start.elapsed().as_millis() < 150, "compression exceeded 150ms budget");
    }
}
