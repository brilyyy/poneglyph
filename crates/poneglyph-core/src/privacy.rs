//! Privacy redaction: strip tagged spans before storage, and refuse to
//! index content that names an excluded path (`.env`, `*.pem`, `secrets/**`).
//! Applied at every ingest boundary (HTTP `/ingest`, CLI `remember`) before
//! the content reaches the store.

use globset::{Glob, GlobSet, GlobSetBuilder};
use regex::RegexBuilder;

use crate::config::PrivacyConfig;

/// Remove all `<tag ...>...</tag>` spans (case-insensitive, multi-line) for
/// each configured redaction tag. Malformed/unterminated tags are left as
/// plain text rather than risk swallowing the rest of the content.
pub fn redact(content: &str, tags: &[String]) -> String {
    let mut out = content.to_string();
    for tag in tags {
        if tag.trim().is_empty() {
            continue;
        }
        let escaped = regex::escape(tag.trim());
        let pattern = format!(r"<{escaped}(?:\s[^>]*)?>.*?</{escaped}\s*>");
        let Ok(re) = RegexBuilder::new(&pattern).case_insensitive(true).dot_matches_new_line(true).build() else {
            continue;
        };
        out = re.replace_all(&out, "").into_owned();
    }
    out
}

/// Build a matcher for `[privacy].exclude_paths` glob patterns. Invalid
/// patterns are skipped rather than failing config load.
pub fn build_exclude_matcher(patterns: &[String]) -> GlobSet {
    let mut builder = GlobSetBuilder::new();
    for p in patterns {
        if let Ok(g) = Glob::new(p) {
            builder.add(g);
        }
    }
    builder.build().unwrap_or_else(|_| GlobSetBuilder::new().build().expect("empty globset always builds"))
}

/// True if any whitespace-delimited token in `content` looks like a path
/// and matches an exclude pattern. Heuristic — ingested content is free
/// text, not a file list, so this catches path-shaped substrings (e.g. a
/// tool-use log line naming `.env`) rather than requiring a dedicated path
/// field.
pub fn content_references_excluded_path(content: &str, matcher: &GlobSet) -> bool {
    if matcher.is_empty() {
        return false;
    }
    content
        .split_whitespace()
        .any(|tok| matcher.is_match(tok.trim_matches(|c: char| c.is_ascii_punctuation() && c != '.' && c != '/')))
}

/// Apply redaction per `[privacy]` config. Path exclusion is a separate,
/// caller-driven check (`content_references_excluded_path`) since it can
/// mean "don't store at all" rather than "strip a span".
pub fn redact_content(content: &str, cfg: &PrivacyConfig) -> String {
    redact(content, &cfg.redaction_tags)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_simple_tag() {
        let out = redact("before <private>secret stuff</private> after", &["private".into()]);
        assert_eq!(out, "before  after");
    }

    #[test]
    fn redacts_case_insensitive_and_with_attrs() {
        let out = redact(r#"x <SECRET level="high">nope</SECRET> y"#, &["secret".into()]);
        assert_eq!(out, "x  y");
    }

    #[test]
    fn redacts_multiline_span() {
        let out = redact("a\n<private>\nline1\nline2\n</private>\nb", &["private".into()]);
        assert_eq!(out, "a\n\nb");
    }

    #[test]
    fn leaves_unmatched_tag_alone() {
        let out = redact("<private>never closed", &["private".into()]);
        assert_eq!(out, "<private>never closed");
    }

    #[test]
    fn leaves_unrelated_text_alone() {
        let out = redact("nothing to redact here", &["private".into(), "secret".into()]);
        assert_eq!(out, "nothing to redact here");
    }

    #[test]
    fn exclude_matcher_blocks_dotenv_and_pem() {
        let cfg = PrivacyConfig::default();
        let matcher = build_exclude_matcher(&cfg.exclude_paths);
        assert!(content_references_excluded_path("Edit .env to add a key", &matcher));
        assert!(content_references_excluded_path("cat server.pem", &matcher));
        assert!(content_references_excluded_path("rm -rf secrets/db.key", &matcher));
        assert!(!content_references_excluded_path("Edit src/main.rs", &matcher));
    }

    #[test]
    fn empty_patterns_never_match() {
        let matcher = build_exclude_matcher(&[]);
        assert!(!content_references_excluded_path("anything at all .env", &matcher));
    }
}
