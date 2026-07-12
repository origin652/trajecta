//! # Contract: diagnostics
//!
//! Diagnostics are deterministic, sortable values used for all user-facing
//! configuration failures. Configuration errors are data; callers must not
//! need to catch panics or parse free-form strings to identify them.
//!
//! ## Exposed interface
//!
//! | Item | Role |
//! |---|---|
//! | [`Severity`] | Error / Warning / Info |
//! | [`PathSegment`] | Field or array index |
//! | [`DiagnosticPath`] | Typed location in the input |
//! | [`Diagnostic`] | Stable code + message + path + hint |
//! | [`DiagnosticBag`] | Mutable merge bag with sorted export |

use std::fmt;

use serde::{Deserialize, Serialize};

/// Severity of a diagnostic.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    /// The requested operation cannot continue.
    Error,
    /// The input is accepted, but deserves attention.
    Warning,
    /// Additional non-problem context.
    Info,
}

/// One component of a typed diagnostic path.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum PathSegment {
    /// A named object field.
    Field(String),
    /// An array index.
    Index(usize),
}

/// Typed path to the input location that produced a diagnostic.
#[derive(Clone, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct DiagnosticPath(Vec<PathSegment>);

impl DiagnosticPath {
    /// Creates an empty root path.
    #[must_use]
    pub const fn root() -> Self {
        Self(Vec::new())
    }

    /// Returns a copy with one field segment appended.
    #[must_use]
    pub fn field(mut self, name: impl Into<String>) -> Self {
        self.0.push(PathSegment::Field(name.into()));
        self
    }

    /// Returns a copy with one index segment appended.
    #[must_use]
    pub fn index(mut self, index: usize) -> Self {
        self.0.push(PathSegment::Index(index));
        self
    }

    /// Returns the ordered path segments.
    #[must_use]
    pub fn segments(&self) -> &[PathSegment] {
        &self.0
    }

    /// Formats the path as `field[0].child` (empty root becomes `$`).
    #[must_use]
    pub fn display_string(&self) -> String {
        if self.0.is_empty() {
            return "$".to_owned();
        }
        let mut out = String::new();
        for (i, segment) in self.0.iter().enumerate() {
            match segment {
                PathSegment::Field(name) => {
                    if i > 0 {
                        out.push('.');
                    }
                    out.push_str(name);
                }
                PathSegment::Index(index) => {
                    out.push('[');
                    out.push_str(&index.to_string());
                    out.push(']');
                }
            }
        }
        out
    }
}

impl fmt::Display for DiagnosticPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.display_string())
    }
}

/// Stable, structured diagnostic returned by validators and resolvers.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct Diagnostic {
    severity: Severity,
    code: String,
    message: String,
    path: DiagnosticPath,
    hint: Option<String>,
}

impl Diagnostic {
    /// Creates an error diagnostic with a stable machine-readable code.
    #[must_use]
    pub fn error(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Error,
            code: code.into(),
            message: message.into(),
            path: DiagnosticPath::root(),
            hint: None,
        }
    }

    /// Creates a warning diagnostic.
    #[must_use]
    pub fn warning(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Warning,
            code: code.into(),
            message: message.into(),
            path: DiagnosticPath::root(),
            hint: None,
        }
    }

    /// Creates an informational diagnostic.
    #[must_use]
    pub fn info(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Info,
            code: code.into(),
            message: message.into(),
            path: DiagnosticPath::root(),
            hint: None,
        }
    }

    /// Assigns a typed input path.
    #[must_use]
    pub fn at(mut self, path: DiagnosticPath) -> Self {
        self.path = path;
        self
    }

    /// Assigns an optional remediation hint.
    #[must_use]
    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    /// Returns the severity.
    #[must_use]
    pub const fn severity(&self) -> Severity {
        self.severity
    }

    /// Returns the stable diagnostic code.
    #[must_use]
    pub fn code(&self) -> &str {
        &self.code
    }

    /// Returns the human-readable message.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }

    /// Returns the typed input path.
    #[must_use]
    pub const fn path(&self) -> &DiagnosticPath {
        &self.path
    }

    /// Returns the remediation hint, when present.
    #[must_use]
    pub fn hint(&self) -> Option<&str> {
        self.hint.as_deref()
    }
}

/// Mutable collection that merges diagnostics before deterministic sorting.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct DiagnosticBag(Vec<Diagnostic>);

impl DiagnosticBag {
    /// Creates an empty bag.
    #[must_use]
    pub const fn new() -> Self {
        Self(Vec::new())
    }

    /// Adds one diagnostic.
    pub fn push(&mut self, diagnostic: Diagnostic) {
        self.0.push(diagnostic);
    }

    /// Extends the bag from another iterator.
    pub fn extend(&mut self, diagnostics: impl IntoIterator<Item = Diagnostic>) {
        self.0.extend(diagnostics);
    }

    /// Appends every diagnostic from another bag.
    pub fn append(&mut self, other: Self) {
        self.0.extend(other.0);
    }

    /// Returns true when at least one error is present.
    #[must_use]
    pub fn has_errors(&self) -> bool {
        self.0
            .iter()
            .any(|diagnostic| diagnostic.severity == Severity::Error)
    }

    /// Returns true when at least one warning is present.
    #[must_use]
    pub fn has_warnings(&self) -> bool {
        self.0
            .iter()
            .any(|diagnostic| diagnostic.severity == Severity::Warning)
    }

    /// Returns a deterministic sorted vector.
    #[must_use]
    pub fn into_sorted(mut self) -> Vec<Diagnostic> {
        self.0.sort();
        self.0
    }

    /// Returns a sorted clone without consuming the bag.
    #[must_use]
    pub fn sorted(&self) -> Vec<Diagnostic> {
        let mut items = self.0.clone();
        items.sort();
        items
    }

    /// Returns the current number of diagnostics.
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Returns true when the bag is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Iterates diagnostics in insertion order.
    pub fn iter(&self) -> impl Iterator<Item = &Diagnostic> {
        self.0.iter()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;

    #[test]
    fn path_builder_and_display() {
        let path = DiagnosticPath::root()
            .field("meteorology")
            .field("domains")
            .index(2)
            .field("id");
        assert_eq!(path.display_string(), "meteorology.domains[2].id");
        assert_eq!(DiagnosticPath::root().display_string(), "$");
        assert_eq!(path.segments().len(), 4);
    }

    #[test]
    fn bag_sorts_deterministically_and_detects_errors() {
        let mut bag = DiagnosticBag::new();
        bag.push(Diagnostic::warning("w", "warn").at(DiagnosticPath::root().field("b")));
        bag.push(Diagnostic::error("e", "err").at(DiagnosticPath::root().field("a")));
        bag.push(Diagnostic::info("i", "info"));
        assert!(bag.has_errors());
        assert!(bag.has_warnings());
        assert_eq!(bag.len(), 3);
        let sorted = bag.into_sorted();
        assert_eq!(sorted[0].severity(), Severity::Error);
        assert_eq!(sorted[0].code(), "e");
    }

    #[test]
    fn diagnostic_accessors_and_hint() {
        let d = Diagnostic::error("code", "message")
            .at(DiagnosticPath::root().field("x"))
            .with_hint("fix it");
        assert_eq!(d.severity(), Severity::Error);
        assert_eq!(d.code(), "code");
        assert_eq!(d.message(), "message");
        assert_eq!(d.path().display_string(), "x");
        assert_eq!(d.hint(), Some("fix it"));
    }

    #[test]
    fn serde_roundtrip() {
        let d = Diagnostic::warning("w", "m").with_hint("h");
        let json = serde_json::to_string(&d).unwrap();
        let back: Diagnostic = serde_json::from_str(&json).unwrap();
        assert_eq!(d, back);
    }
}
