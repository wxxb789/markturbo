//! Diagnostics attached to a document or an individual block.
//!
//! Renderer failures must never crash the app; they land here and are shown
//! inline next to the block that produced them, with the original source
//! preserved.

use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Error,
    Warning,
    Info,
}

impl fmt::Display for Severity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Severity::Error => "error",
            Severity::Warning => "warning",
            Severity::Info => "info",
        })
    }
}

/// A problem found in a document, optionally anchored to a source line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub severity: Severity,
    /// Which subsystem produced it, e.g. `"mermaid"`, `"frontmatter"`, `"mdx"`.
    pub source: String,
    pub message: String,
    /// 1-based line in the document, when known.
    pub line: Option<usize>,
}

impl Diagnostic {
    pub fn error(source: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Error,
            source: source.into(),
            message: message.into(),
            line: None,
        }
    }

    pub fn warning(source: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Warning,
            source: source.into(),
            message: message.into(),
            line: None,
        }
    }

    pub fn info(source: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Info,
            source: source.into(),
            message: message.into(),
            line: None,
        }
    }

    pub fn at_line(mut self, line: usize) -> Self {
        self.line = Some(line);
        self
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.line {
            Some(line) => write!(
                f,
                "{}:{}: {}: {}",
                self.source, line, self.severity, self.message
            ),
            None => write!(f, "{}: {}: {}", self.source, self.severity, self.message),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_includes_line_when_known() {
        let d = Diagnostic::error("mermaid", "Unexpected token").at_line(7);
        assert_eq!(d.to_string(), "mermaid:7: error: Unexpected token");
        let d = Diagnostic::warning("frontmatter", "missing name");
        assert_eq!(d.to_string(), "frontmatter: warning: missing name");
    }

    #[test]
    fn severity_orders_error_first() {
        let mut v = vec![Severity::Info, Severity::Error, Severity::Warning];
        v.sort();
        assert_eq!(v, vec![Severity::Error, Severity::Warning, Severity::Info]);
    }
}
