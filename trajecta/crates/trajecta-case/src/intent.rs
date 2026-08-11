//! # Contract: validation intent
//!
//! Component presence is validated against the operation the caller wants to
//! perform. A partially specified Case can be valid for meteorology probing
//! while remaining invalid for a full simulation.
//!
//! ## Exposed interface
//!
//! | Item | Role |
//! |---|---|
//! | [`ValidationIntent`] | MetProbe / MetReplay / Simulation / Migration |
//! | [`CaseRequirements`] | Presence requirements for an intent |
//! | [`IntentValidator`] | Deterministic presence checks |
//!
//! ## Requirements matrix
//!
//! | Intent | time | meteorology | population | numerics |
//! |---|---|---|---|---|
//! | MetProbe | yes | yes | no | no |
//! | MetReplay | yes | yes | no | no |
//! | Simulation | yes | yes | yes | yes |
//! | Migration | no | no | no | no |

use serde::{Deserialize, Serialize};

use crate::diagnostic::{Diagnostic, DiagnosticBag, DiagnosticPath};
use crate::document::ResolvedCase;

/// Operation for which a resolved Case is being validated.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValidationIntent {
    /// Inspect meteorological availability and metadata.
    MetProbe,
    /// Query and emit a deterministic meteorology replay.
    MetReplay,
    /// Execute particle transport.
    Simulation,
    /// Convert a legacy configuration without executing it.
    Migration,
}

/// Component and capability requirements associated with an intent.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct CaseRequirements {
    /// Whether a time specification is required.
    pub time: bool,
    /// Whether a meteorology specification is required.
    pub meteorology: bool,
    /// Whether a particle-population strategy is required.
    pub particle_population: bool,
    /// Whether numerical controls are required.
    pub numerics: bool,
    /// Stable meteorology capability names required by the selected modules.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub meteorology_capabilities: Vec<String>,
}

impl CaseRequirements {
    /// Returns the baseline requirements for a validation intent.
    #[must_use]
    pub fn for_intent(intent: ValidationIntent) -> Self {
        match intent {
            ValidationIntent::MetProbe | ValidationIntent::MetReplay => Self {
                time: true,
                meteorology: true,
                ..Self::default()
            },
            ValidationIntent::Simulation => Self {
                time: true,
                meteorology: true,
                particle_population: true,
                numerics: true,
                meteorology_capabilities: Vec::new(),
            },
            ValidationIntent::Migration => Self::default(),
        }
    }
}

/// Deterministic validator for operation-specific component presence.
#[derive(Clone, Copy, Debug, Default)]
pub struct IntentValidator;

impl IntentValidator {
    /// Validates only operation-specific presence requirements.
    #[must_use]
    pub fn validate(case: &ResolvedCase, intent: ValidationIntent) -> DiagnosticBag {
        let requirements = CaseRequirements::for_intent(intent);
        let mut diagnostics = DiagnosticBag::new();
        if requirements.time && case.time.is_none() {
            diagnostics.push(
                Diagnostic::error("case.missing_time", "time is required for this operation")
                    .at(DiagnosticPath::root().field("time")),
            );
        }
        if requirements.meteorology && case.meteorology.is_none() {
            diagnostics.push(
                Diagnostic::error(
                    "case.missing_meteorology",
                    "meteorology is required for this operation",
                )
                .at(DiagnosticPath::root().field("meteorology")),
            );
        }
        if requirements.particle_population && case.particle_population.is_none() {
            diagnostics.push(
                Diagnostic::error(
                    "case.missing_particle_population",
                    "particle_population is required for this operation",
                )
                .at(DiagnosticPath::root().field("particle_population")),
            );
        }
        if requirements.numerics && case.numerics.is_none() {
            diagnostics.push(
                Diagnostic::error(
                    "case.missing_numerics",
                    "numerics is required for this operation",
                )
                .at(DiagnosticPath::root().field("numerics")),
            );
        }
        diagnostics
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use crate::document::ResolvedCase;
    use crate::model::metadata::Metadata;
    use crate::model::meteorology::MeteorologySpec;
    use crate::model::time::{Direction, TimeSpec, Timestamp};

    fn empty_case() -> ResolvedCase {
        ResolvedCase {
            metadata: Metadata::default(),
            time: None,
            meteorology: None,
            particle_population: None,
            substances: Vec::new(),
            numerics: None,
            physics: None,
            outputs: Vec::new(),
            sources: Vec::new(),
        }
    }

    #[test]
    fn simulation_requires_four_components() {
        let diagnostics = IntentValidator::validate(&empty_case(), ValidationIntent::Simulation);
        assert_eq!(diagnostics.len(), 4);
        assert!(diagnostics.has_errors());
    }

    #[test]
    fn met_probe_accepts_time_and_meteorology_only() {
        let mut case = empty_case();
        case.time = Some(TimeSpec {
            start: Timestamp::UNIX_EPOCH,
            end: Timestamp::new(1, 0).unwrap(),
            direction: Direction::Forward,
        });
        case.meteorology = Some(MeteorologySpec::default());
        let diagnostics = IntentValidator::validate(&case, ValidationIntent::MetProbe);
        assert!(!diagnostics.has_errors());
        assert!(diagnostics.is_empty());
    }

    #[test]
    fn migration_has_no_presence_requirements() {
        let diagnostics = IntentValidator::validate(&empty_case(), ValidationIntent::Migration);
        assert!(diagnostics.is_empty());
        let req = CaseRequirements::for_intent(ValidationIntent::Migration);
        assert!(!req.time && !req.meteorology && !req.particle_population && !req.numerics);
    }
}
