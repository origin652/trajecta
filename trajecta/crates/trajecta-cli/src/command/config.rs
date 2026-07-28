//! # Contract: machine configuration commands
//!
//! Typed command selections for the local, schema-versioned machine configuration.

/// Machine configuration command selection.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ConfigCommand {
    /// Create a default configuration without replacing an existing file.
    Init,
    /// Print the selected configuration path.
    Path,
    /// List leaf values in deterministic dotted-key order.
    List,
    /// Read one typed dotted-key value.
    Get {
        /// Existing dotted key.
        key: String,
    },
    /// Atomically set one existing dotted-key value.
    Set {
        /// Existing dotted key.
        key: String,
        /// JSON value text or a string fallback.
        value: String,
    },
    /// Remove a template entry or reject removal of a required value.
    Unset {
        /// Existing dotted key.
        key: String,
    },
    /// Validate the selected configuration.
    Validate,
}
