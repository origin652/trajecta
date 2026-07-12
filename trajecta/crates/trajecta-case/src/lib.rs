//! # Contract: Case and RunProfile crate
//!
//! This crate owns configuration shape, units, local references, validation
//! intent, and immutable dataset identities. It deliberately knows nothing
//! about meteorological decoding or particle execution.
//!
//! ## Modules for downstream AI / crates
//!
//! | Module | Responsibility | Key types |
//! |---|---|---|
//! | [`diagnostic`] | Typed, sortable configuration diagnostics | `Diagnostic`, `DiagnosticPath` |
//! | [`document`] | Case / RunProfile / resolved forms | `CaseDocument`, `ResolvedCase` |
//! | [`expand`] | Single-level component expansion + machine paths | `expand_case_file`, `expand_run_profile_file` |
//! | [`intent`] | Operation-specific presence validation | `ValidationIntent`, `IntentValidator` |
//! | [`lockfile`] | Immutable dataset locks | `DatasetLock`, `LockedFile` |
//! | [`model`] | Scientific component specs | `TimeSpec`, `MeteorologySpec`, … |
//! | [`quantity`] | Units and SI-normalized quantities | `Quantity`, `UnitRegistry` |
//! | [`mod@reference`] | Safe local component refs | [`ComponentRef`](crate::reference::ComponentRef), [`RefPath`](crate::reference::RefPath) |
//! | [`resolver`] | Local file resolution + digests | `LocalRefResolver`, `SourceDigest` |
//! | [`schema`] | Parse + shape validation | `SchemaDocument`, `parse_case_yaml` |
//!
//! See each module's rustdoc for field-level contracts.
//!
//! Human-readable summary for other implementers: `crates/trajecta-case/API.md`.

#![deny(missing_docs)]
#![forbid(unsafe_code)]

pub mod diagnostic;
pub mod document;
pub mod expand;
pub mod intent;
pub mod lockfile;
pub mod model;
pub mod quantity;
pub mod reference;
pub mod resolver;
pub mod schema;

/// Version of the compiler-checked public code contracts.
pub const CONTRACT_VERSION: u32 = 0;
