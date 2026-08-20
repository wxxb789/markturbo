//! YAML frontmatter extraction.
//!
//! Kept separate from the Markdown parse because several consumers (skill
//! discovery, document-type labeling, the future instruction resolver) need the
//! metadata without paying for a full AST.

use crate::diagnostic::Diagnostic;

/// A frontmatter block located in a source document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frontmatter {
    /// Raw YAML text, without the fences.
    pub raw: String,
    /// Byte offset of the YAML body in the source.
    pub body_start: usize,
    /// Byte offset just past the closing fence (start of the Markdown body).
    pub content_start: usize,
}

/// Split a document into its frontmatter (if any) and the remaining content.
///
/// Only a leading `---` fence counts, per the convention every agent tool
/// implements. A file that merely contains `---` later is untouched.
pub fn split(source: &str) -> (Option<Frontmatter>, &str) {
    // A BOM is common in files written by Windows editors; skip it so the
    // fence check still matches.
    let body = source.strip_prefix('\u{feff}').unwrap_or(source);
    let bom_len = source.len() - body.len();

    let Some(rest) = strip_open_fence(body) else {
        return (None, source);
    };
    let body_start = bom_len + (body.len() - rest.len());

    let mut offset = body_start;
    for line in rest.split_inclusive('\n') {
        let trimmed = line.trim_end_matches(['\n', '\r']);
        if trimmed == "---" || trimmed == "..." {
            let yaml_end = offset;
            let content_start = offset + line.len();
            return (
                Some(Frontmatter {
                    raw: source[body_start..yaml_end].to_string(),
                    body_start,
                    content_start,
                }),
                &source[content_start..],
            );
        }
        offset += line.len();
    }

    // Unterminated fence: treat the whole file as content rather than
    // swallowing it, and let the caller report the problem.
    (None, source)
}

/// Consume a leading `---` line, returning the text after it.
fn strip_open_fence(source: &str) -> Option<&str> {
    let rest = source.strip_prefix("---")?;
    // Must be `---` alone on the first line, otherwise this is a thematic break
    // or a setext heading underline.
    let rest = rest.strip_prefix("\r\n").or_else(|| rest.strip_prefix('\n'))?;
    Some(rest)
}

/// Parse frontmatter YAML into a value, reporting failures as diagnostics
/// rather than errors: a malformed header must not stop the document opening.
pub fn parse_yaml(raw: &str) -> Result<serde_yaml::Value, Diagnostic> {
    match serde_yaml::from_str::<serde_yaml::Value>(raw) {
        Ok(serde_yaml::Value::Null) => Ok(serde_yaml::Value::Mapping(Default::default())),
        Ok(value) => Ok(value),
        Err(err) => {
            let mut diag = Diagnostic::error("frontmatter", err.to_string());
            if let Some(loc) = err.location() {
                // +1: YAML line numbers are relative to the block, the document
                // has the opening `---` above it.
                diag = diag.at_line(loc.line() + 1);
            }
            Err(diag)
        }
    }
}

/// True when an unterminated `---` fence opens the document.
///
/// `split` deliberately returns the whole file in this case; this predicate
/// lets the caller add a diagnostic without duplicating the scan logic.
pub fn has_unterminated_fence(source: &str) -> bool {
    let body = source.strip_prefix('\u{feff}').unwrap_or(source);
    let Some(rest) = strip_open_fence(body) else {
        return false;
    };
    !rest
        .lines()
        .any(|l| l.trim_end_matches('\r') == "---" || l.trim_end_matches('\r') == "...")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_frontmatter() {
        let src = "---\nname: demo\ndescription: A demo\n---\n# Title\n";
        let (fm, content) = split(src);
        let fm = fm.expect("frontmatter");
        assert_eq!(fm.raw, "name: demo\ndescription: A demo\n");
        assert_eq!(content, "# Title\n");
    }

    #[test]
    fn handles_crlf_and_bom() {
        let src = "\u{feff}---\r\nname: demo\r\n---\r\n# Title\r\n";
        let (fm, content) = split(src);
        let fm = fm.expect("frontmatter");
        assert_eq!(fm.raw, "name: demo\r\n");
        assert_eq!(content, "# Title\r\n");
    }

    #[test]
    fn no_frontmatter_leaves_source_untouched() {
        let src = "# Title\n\n---\n\nnot frontmatter\n";
        let (fm, content) = split(src);
        assert!(fm.is_none());
        assert_eq!(content, src);
    }

    #[test]
    fn thematic_break_is_not_a_fence() {
        // `--- ` with trailing content on the same line is not an open fence.
        let src = "--- not a fence\nbody\n";
        assert!(split(src).0.is_none());
    }

    #[test]
    fn unterminated_fence_is_detected_and_content_preserved() {
        let src = "---\nname: demo\n# Title\n";
        let (fm, content) = split(src);
        assert!(fm.is_none());
        assert_eq!(content, src, "content must never be lost");
        assert!(has_unterminated_fence(src));
    }

    #[test]
    fn empty_frontmatter_parses_to_empty_mapping() {
        let value = parse_yaml("").expect("empty yaml is valid");
        assert!(value.as_mapping().is_some_and(|m| m.is_empty()));
    }

    #[test]
    fn malformed_yaml_becomes_a_diagnostic_not_a_panic() {
        let err = parse_yaml("name: [unclosed\n").expect_err("should fail");
        assert_eq!(err.source, "frontmatter");
    }

    #[test]
    fn offsets_point_at_real_slices() {
        let src = "---\nname: demo\n---\nbody\n";
        let (fm, content) = split(src);
        let fm = fm.unwrap();
        assert_eq!(&src[fm.body_start..fm.body_start + fm.raw.len()], fm.raw);
        assert_eq!(&src[fm.content_start..], content);
    }
}
