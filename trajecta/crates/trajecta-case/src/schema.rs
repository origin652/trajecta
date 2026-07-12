//! # Contract: document schema boundary
//!
//! Schema version zero is a development format. A document must identify its
//! kind explicitly; shape validation is separate from intent and runtime
//! capability validation.
//!
//! ## Exposed interface
//!
//! | Item | Role |
//! |---|---|
//! | [`CURRENT_SCHEMA_VERSION`] | Development schema version (`0`) |
//! | [`SchemaDocument`] | Shape-validation contract |
//! | [`parse_case_yaml`] / [`parse_case_json`] | Case loaders |
//! | [`parse_run_profile_yaml`] / [`parse_run_profile_json`] | Profile loaders |
//! | [`validate_resolved_case`] | Shape checks for fully expanded Case |
//! | [`SchemaError`] | Parse / kind / version failures |

use std::collections::{BTreeMap, BTreeSet};

use crate::diagnostic::{Diagnostic, DiagnosticBag, DiagnosticPath};
use crate::document::{
    CaseDocument, DocumentKind, ExecutionSpec, ResolvedCase, RunProfileDocument,
};
use crate::model::meteorology::{DomainId, MeteorologySpec};
use crate::model::numerics::NumericsSpec;
use crate::model::output::OutputProductSpec;
use crate::model::physics::PhysicsModuleSpec;
use crate::model::population::{DomainFillAirMassSpec, ParticlePopulationSpec};
use crate::model::substance::SubstanceSpec;
use crate::model::time::{Direction, TimeSpec};
use crate::quantity::Quantity;
use crate::reference::ComponentRef;

/// Current development-stage Case and RunProfile schema version.
pub const CURRENT_SCHEMA_VERSION: u32 = 0;

/// Common contract implemented by top-level schema documents.
pub trait SchemaDocument {
    /// Declared schema version.
    fn schema_version(&self) -> u32;

    /// Declared top-level kind.
    fn document_kind(&self) -> DocumentKind;

    /// Performs shape-only validation without file or network access.
    fn validate_shape(&self) -> Result<DiagnosticBag, SchemaError>;
}

impl SchemaDocument for CaseDocument {
    fn schema_version(&self) -> u32 {
        self.schema_version
    }

    fn document_kind(&self) -> DocumentKind {
        self.kind
    }

    fn validate_shape(&self) -> Result<DiagnosticBag, SchemaError> {
        if self.schema_version != CURRENT_SCHEMA_VERSION {
            return Err(SchemaError::UnsupportedVersion(self.schema_version));
        }
        if self.kind != DocumentKind::Case {
            return Err(SchemaError::WrongDocumentKind(self.kind));
        }

        let mut diagnostics = DiagnosticBag::new();
        if self.metadata.name.trim().is_empty() {
            diagnostics.push(
                Diagnostic::error(
                    "case.metadata.name_empty",
                    "metadata.name must not be empty",
                )
                .at(DiagnosticPath::root().field("metadata").field("name")),
            );
        }
        if let Some(ComponentRef::Inline(time)) = &self.time {
            validate_time_spec(time, DiagnosticPath::root().field("time"), &mut diagnostics);
        }
        if let Some(ComponentRef::Inline(met)) = &self.meteorology {
            validate_meteorology(
                met,
                DiagnosticPath::root().field("meteorology"),
                &mut diagnostics,
            );
        }
        if let Some(ComponentRef::Inline(pop)) = &self.particle_population {
            validate_population(
                pop,
                DiagnosticPath::root().field("particle_population"),
                &mut diagnostics,
            );
        }
        if let Some(ComponentRef::Inline(substances)) = &self.substances {
            validate_substances(
                substances,
                DiagnosticPath::root().field("substances"),
                &mut diagnostics,
            );
        }
        if let Some(ComponentRef::Inline(numerics)) = &self.numerics {
            validate_numerics(
                numerics,
                DiagnosticPath::root().field("numerics"),
                &mut diagnostics,
            );
        }
        if let Some(ComponentRef::Inline(physics)) = &self.physics {
            validate_physics(
                physics,
                DiagnosticPath::root().field("physics"),
                &mut diagnostics,
            );
        }
        if let Some(ComponentRef::Inline(outputs)) = &self.outputs {
            validate_outputs(
                outputs,
                DiagnosticPath::root().field("outputs"),
                &mut diagnostics,
            );
        }
        Ok(diagnostics)
    }
}

/// Validates a fully expanded Case after component references are resolved.
///
/// Unlike [`CaseDocument::validate_shape`], this inspects concrete component
/// values whether they originally arrived inline or via a single-level `ref`.
#[must_use]
pub fn validate_resolved_case(case: &ResolvedCase) -> DiagnosticBag {
    let mut diagnostics = DiagnosticBag::new();
    if case.metadata.name.trim().is_empty() {
        diagnostics.push(
            Diagnostic::error(
                "case.metadata.name_empty",
                "metadata.name must not be empty",
            )
            .at(DiagnosticPath::root().field("metadata").field("name")),
        );
    }
    if let Some(time) = &case.time {
        validate_time_spec(time, DiagnosticPath::root().field("time"), &mut diagnostics);
    }
    if let Some(met) = &case.meteorology {
        validate_meteorology(
            met,
            DiagnosticPath::root().field("meteorology"),
            &mut diagnostics,
        );
    }
    if let Some(pop) = &case.particle_population {
        validate_population(
            pop,
            DiagnosticPath::root().field("particle_population"),
            &mut diagnostics,
        );
    }
    validate_substances(
        &case.substances,
        DiagnosticPath::root().field("substances"),
        &mut diagnostics,
    );
    if let Some(numerics) = &case.numerics {
        validate_numerics(
            numerics,
            DiagnosticPath::root().field("numerics"),
            &mut diagnostics,
        );
    }
    validate_physics(
        &case.physics,
        DiagnosticPath::root().field("physics"),
        &mut diagnostics,
    );
    validate_outputs(
        &case.outputs,
        DiagnosticPath::root().field("outputs"),
        &mut diagnostics,
    );
    diagnostics
}

impl SchemaDocument for RunProfileDocument {
    fn schema_version(&self) -> u32 {
        self.schema_version
    }

    fn document_kind(&self) -> DocumentKind {
        self.kind
    }

    fn validate_shape(&self) -> Result<DiagnosticBag, SchemaError> {
        if self.schema_version != CURRENT_SCHEMA_VERSION {
            return Err(SchemaError::UnsupportedVersion(self.schema_version));
        }
        if self.kind != DocumentKind::RunProfile {
            return Err(SchemaError::WrongDocumentKind(self.kind));
        }

        let mut diagnostics = DiagnosticBag::new();
        if self.metadata.name.trim().is_empty() {
            diagnostics.push(
                Diagnostic::error(
                    "run_profile.metadata.name_empty",
                    "metadata.name must not be empty",
                )
                .at(DiagnosticPath::root().field("metadata").field("name")),
            );
        }
        if self.case_path.as_os_str().is_empty() {
            diagnostics.push(
                Diagnostic::error("run_profile.case_path_empty", "case_path must not be empty")
                    .at(DiagnosticPath::root().field("case_path")),
            );
        }
        validate_execution(
            &self.execution,
            DiagnosticPath::root().field("execution"),
            &mut diagnostics,
        );
        let mut seen_datasets = BTreeSet::new();
        for (index, binding) in self.datasets.iter().enumerate() {
            if binding.dataset.0.trim().is_empty() {
                diagnostics.push(
                    Diagnostic::error(
                        "run_profile.dataset_empty",
                        "dataset identifier must not be empty",
                    )
                    .at(DiagnosticPath::root()
                        .field("datasets")
                        .index(index)
                        .field("dataset")),
                );
            } else if !seen_datasets.insert(binding.dataset.0.clone()) {
                diagnostics.push(
                    Diagnostic::error(
                        "run_profile.dataset_duplicate",
                        format!("duplicate dataset binding '{}'", binding.dataset.0),
                    )
                    .at(DiagnosticPath::root()
                        .field("datasets")
                        .index(index)
                        .field("dataset")),
                );
            }
            if binding.lockfile.as_os_str().is_empty() {
                diagnostics.push(
                    Diagnostic::error(
                        "run_profile.lockfile_empty",
                        "dataset lockfile path must not be empty",
                    )
                    .at(DiagnosticPath::root()
                        .field("datasets")
                        .index(index)
                        .field("lockfile")),
                );
            }
        }
        Ok(diagnostics)
    }
}

fn validate_time_spec(time: &TimeSpec, path: DiagnosticPath, diagnostics: &mut DiagnosticBag) {
    let ordered = match time.direction {
        Direction::Forward => time.start <= time.end,
        Direction::Backward => time.start >= time.end,
    };
    if !ordered {
        diagnostics.push(
            Diagnostic::error(
                "case.time.range_inconsistent",
                "time range is inconsistent with direction",
            )
            .at(path)
            .with_hint("for forward runs start <= end; for backward runs start >= end"),
        );
    }
}

fn validate_meteorology(
    met: &MeteorologySpec,
    path: DiagnosticPath,
    diagnostics: &mut DiagnosticBag,
) {
    if met.domains.is_empty() {
        diagnostics.push(
            Diagnostic::error(
                "case.meteorology.domains_empty",
                "meteorology.domains must contain at least one domain",
            )
            .at(path.field("domains")),
        );
        return;
    }

    let mut ids = BTreeMap::<String, usize>::new();
    for (index, domain) in met.domains.iter().enumerate() {
        let domain_path = path.clone().field("domains").index(index);
        if domain.id.0.trim().is_empty() {
            diagnostics.push(
                Diagnostic::error(
                    "case.meteorology.domain_id_empty",
                    "domain id must not be empty",
                )
                .at(domain_path.clone().field("id")),
            );
        } else if let Some(previous) = ids.insert(domain.id.0.clone(), index) {
            diagnostics.push(
                Diagnostic::error(
                    "case.meteorology.domain_id_duplicate",
                    format!(
                        "duplicate domain id '{}' (also at index {previous})",
                        domain.id.0
                    ),
                )
                .at(domain_path.clone().field("id")),
            );
        }
        if domain.dataset.0.trim().is_empty() {
            diagnostics.push(
                Diagnostic::error(
                    "case.meteorology.dataset_empty",
                    "domain dataset must not be empty",
                )
                .at(domain_path.clone().field("dataset")),
            );
        }
        if domain.horizontal_halo_cells == 0 {
            diagnostics.push(
                Diagnostic::error(
                    "case.meteorology.halo_zero",
                    "horizontal_halo_cells must be >= 1",
                )
                .at(domain_path.clone().field("horizontal_halo_cells")),
            );
        }
        if let Some(parent) = &domain.parent {
            if parent.0.trim().is_empty() {
                diagnostics.push(
                    Diagnostic::error(
                        "case.meteorology.parent_empty",
                        "domain parent id must not be empty",
                    )
                    .at(domain_path.clone().field("parent")),
                );
            } else if parent == &domain.id {
                diagnostics.push(
                    Diagnostic::error(
                        "case.meteorology.parent_self",
                        "domain parent must not point to itself",
                    )
                    .at(domain_path.field("parent")),
                );
            }
        }
    }

    // Parent existence and cycle detection.
    let id_set: BTreeSet<String> = met.domains.iter().map(|d| d.id.0.clone()).collect();
    let mut parent_of: BTreeMap<String, String> = BTreeMap::new();
    for (index, domain) in met.domains.iter().enumerate() {
        if let Some(parent) = &domain.parent {
            if !parent.0.trim().is_empty() && parent != &domain.id && !id_set.contains(&parent.0) {
                diagnostics.push(
                    Diagnostic::error(
                        "case.meteorology.parent_missing",
                        format!("domain parent '{}' does not exist", parent.0),
                    )
                    .at(path
                        .clone()
                        .field("domains")
                        .index(index)
                        .field("parent")),
                );
            } else if !parent.0.trim().is_empty() && parent != &domain.id {
                parent_of.insert(domain.id.0.clone(), parent.0.clone());
            }
        }
    }
    for domain in &met.domains {
        if let Some(cycle) = detect_parent_cycle(&domain.id, &parent_of) {
            diagnostics.push(
                Diagnostic::error(
                    "case.meteorology.parent_cycle",
                    format!("domain parent cycle detected: {}", cycle.join(" -> ")),
                )
                .at(path.clone().field("domains")),
            );
            break;
        }
    }
}

fn detect_parent_cycle(
    start: &DomainId,
    parent_of: &BTreeMap<String, String>,
) -> Option<Vec<String>> {
    let mut seen = Vec::new();
    let mut current = start.0.as_str();
    let mut guard = BTreeSet::new();
    while let Some(parent) = parent_of.get(current) {
        seen.push(current.to_owned());
        if !guard.insert(current.to_owned()) {
            seen.push(parent.clone());
            return Some(seen);
        }
        if parent == &start.0 && !seen.is_empty() {
            seen.push(parent.clone());
            return Some(seen);
        }
        current = parent.as_str();
        if seen.len() > parent_of.len() + 1 {
            seen.push(current.to_owned());
            return Some(seen);
        }
    }
    None
}

fn validate_numerics(
    numerics: &NumericsSpec,
    path: DiagnosticPath,
    diagnostics: &mut DiagnosticBag,
) {
    if numerics.integrator.model.0.trim().is_empty() {
        diagnostics.push(
            Diagnostic::error(
                "case.numerics.integrator_empty",
                "numerics.integrator.model must not be empty",
            )
            .at(path.clone().field("integrator").field("model")),
        );
    }
    if !numerics.time_step.is_positive_finite() {
        diagnostics.push(
            Diagnostic::error(
                "case.numerics.time_step_invalid",
                "numerics.time_step must be a positive finite duration",
            )
            .at(path.clone().field("time_step")),
        );
    }
    for (index, policy) in numerics.boundaries.policies.iter().enumerate() {
        if policy.0.trim().is_empty() {
            diagnostics.push(
                Diagnostic::error(
                    "case.numerics.boundary_empty",
                    "boundary policy model id must not be empty",
                )
                .at(path
                    .clone()
                    .field("boundaries")
                    .field("policies")
                    .index(index)),
            );
        }
    }
}

fn validate_population(
    pop: &ParticlePopulationSpec,
    path: DiagnosticPath,
    diagnostics: &mut DiagnosticBag,
) {
    match pop {
        ParticlePopulationSpec::ReleaseDriven(spec) => {
            if spec.id.0.trim().is_empty() {
                diagnostics.push(
                    Diagnostic::error(
                        "case.population.id_empty",
                        "population id must not be empty",
                    )
                    .at(path.clone().field("id")),
                );
            }
            if spec.schedule.trim().is_empty() {
                diagnostics.push(
                    Diagnostic::error(
                        "case.population.schedule_empty",
                        "release schedule id must not be empty",
                    )
                    .at(path.field("schedule")),
                );
            }
        }
        ParticlePopulationSpec::DomainFillAirMass(spec) => {
            validate_domain_fill_air_mass(spec, path, diagnostics);
        }
        ParticlePopulationSpec::DomainFillStratosphericOzone(spec) => {
            validate_domain_fill_air_mass(
                &spec.air_mass,
                path.clone().field("air_mass"),
                diagnostics,
            );
            if spec.ozone_rule.trim().is_empty() {
                diagnostics.push(
                    Diagnostic::error(
                        "case.population.ozone_rule_empty",
                        "ozone_rule must not be empty",
                    )
                    .at(path.field("ozone_rule")),
                );
            }
        }
    }
}

fn validate_domain_fill_air_mass(
    spec: &DomainFillAirMassSpec,
    path: DiagnosticPath,
    diagnostics: &mut DiagnosticBag,
) {
    if spec.id.0.trim().is_empty() {
        diagnostics.push(
            Diagnostic::error(
                "case.population.id_empty",
                "population id must not be empty",
            )
            .at(path.clone().field("id")),
        );
    }
    match (&spec.target_particle_mass, &spec.target_particle_count) {
        (None, None) => {
            diagnostics.push(
                Diagnostic::error(
                    "case.population.target_missing",
                    "domain-fill requires exactly one of target_particle_mass or target_particle_count",
                )
                .at(path),
            );
        }
        (Some(_), Some(_)) => {
            diagnostics.push(
                Diagnostic::error(
                    "case.population.target_both",
                    "domain-fill must not set both target_particle_mass and target_particle_count",
                )
                .at(path),
            );
        }
        (Some(mass), None) => {
            if !mass.is_positive_finite() {
                diagnostics.push(
                    Diagnostic::error(
                        "case.population.target_mass_invalid",
                        "target_particle_mass must be a finite positive mass",
                    )
                    .at(path.field("target_particle_mass")),
                );
            }
        }
        (None, Some(count)) => {
            if *count == 0 {
                diagnostics.push(
                    Diagnostic::error(
                        "case.population.target_count_zero",
                        "target_particle_count must be > 0",
                    )
                    .at(path.field("target_particle_count")),
                );
            }
        }
    }
}

fn validate_substances(
    substances: &[SubstanceSpec],
    path: DiagnosticPath,
    diagnostics: &mut DiagnosticBag,
) {
    let mut seen = BTreeSet::new();
    for (index, substance) in substances.iter().enumerate() {
        if substance.id.0.trim().is_empty() {
            diagnostics.push(
                Diagnostic::error("case.substance.id_empty", "substance id must not be empty")
                    .at(path.clone().index(index).field("id")),
            );
        } else if !seen.insert(substance.id.0.clone()) {
            diagnostics.push(
                Diagnostic::error(
                    "case.substance.id_duplicate",
                    format!("duplicate substance id '{}'", substance.id.0),
                )
                .at(path.clone().index(index).field("id")),
            );
        }
    }
}

fn validate_physics(
    physics: &[PhysicsModuleSpec],
    path: DiagnosticPath,
    diagnostics: &mut DiagnosticBag,
) {
    for (index, module) in physics.iter().enumerate() {
        if module.model.0.trim().is_empty() {
            diagnostics.push(
                Diagnostic::error(
                    "case.physics.model_empty",
                    "physics module model id must not be empty",
                )
                .at(path.clone().index(index).field("model")),
            );
        }
    }
}

fn validate_outputs(
    outputs: &[OutputProductSpec],
    path: DiagnosticPath,
    diagnostics: &mut DiagnosticBag,
) {
    let mut seen = BTreeSet::new();
    for (index, product) in outputs.iter().enumerate() {
        let product_path = path.clone().index(index);
        if product.product.0.trim().is_empty() {
            diagnostics.push(
                Diagnostic::error(
                    "case.output.product_empty",
                    "output product id must not be empty",
                )
                .at(product_path.clone().field("product")),
            );
        } else if !seen.insert(product.product.0.clone()) {
            diagnostics.push(
                Diagnostic::error(
                    "case.output.product_duplicate",
                    format!("duplicate output product id '{}'", product.product.0),
                )
                .at(product_path.clone().field("product")),
            );
        }
        if product.encoder.model.0.trim().is_empty() {
            diagnostics.push(
                Diagnostic::error(
                    "case.output.encoder_empty",
                    "output encoder model id must not be empty",
                )
                .at(product_path.clone().field("encoder").field("model")),
            );
        }
        validate_positive_duration(
            &product.schedule.interval,
            product_path.clone().field("schedule").field("interval"),
            "case.output.interval_invalid",
            "output interval must be a finite positive duration",
            diagnostics,
        );
        if let Some(avg) = &product.schedule.averaging_interval {
            validate_positive_duration(
                avg,
                product_path.field("schedule").field("averaging_interval"),
                "case.output.averaging_interval_invalid",
                "output averaging_interval must be a finite positive duration",
                diagnostics,
            );
        }
    }
}

fn validate_positive_duration<D: crate::quantity::DimensionMarker>(
    quantity: &Quantity<D>,
    path: DiagnosticPath,
    code: &str,
    message: &str,
    diagnostics: &mut DiagnosticBag,
) {
    if !quantity.is_positive_finite() {
        diagnostics.push(Diagnostic::error(code, message).at(path));
    }
}

fn validate_execution(
    execution: &ExecutionSpec,
    path: DiagnosticPath,
    diagnostics: &mut DiagnosticBag,
) {
    if execution.worker_threads == 0 {
        diagnostics.push(
            Diagnostic::error(
                "run_profile.worker_threads_zero",
                "execution.worker_threads must be >= 1",
            )
            .at(path.clone().field("worker_threads")),
        );
    }
    if execution.memory_budget_bytes == 0 {
        diagnostics.push(
            Diagnostic::error(
                "run_profile.memory_budget_zero",
                "execution.memory_budget_bytes must be >= 1",
            )
            .at(path.clone().field("memory_budget_bytes")),
        );
    }
    if execution.executor.trim().is_empty() {
        diagnostics.push(
            Diagnostic::error(
                "run_profile.executor_empty",
                "execution.executor must not be empty",
            )
            .at(path.field("executor")),
        );
    }
}

/// Parses a Case document from YAML text.
pub fn parse_case_yaml(input: &str) -> Result<CaseDocument, SchemaError> {
    let doc: CaseDocument =
        serde_yml::from_str(input).map_err(|error| SchemaError::Parse(error.to_string()))?;
    ensure_kind(doc.kind, DocumentKind::Case)?;
    ensure_version(doc.schema_version)?;
    Ok(doc)
}

/// Parses a Case document from JSON text.
pub fn parse_case_json(input: &str) -> Result<CaseDocument, SchemaError> {
    let doc: CaseDocument =
        serde_json::from_str(input).map_err(|error| SchemaError::Parse(error.to_string()))?;
    ensure_kind(doc.kind, DocumentKind::Case)?;
    ensure_version(doc.schema_version)?;
    Ok(doc)
}

/// Parses a RunProfile document from YAML text.
pub fn parse_run_profile_yaml(input: &str) -> Result<RunProfileDocument, SchemaError> {
    let doc: RunProfileDocument =
        serde_yml::from_str(input).map_err(|error| SchemaError::Parse(error.to_string()))?;
    ensure_kind(doc.kind, DocumentKind::RunProfile)?;
    ensure_version(doc.schema_version)?;
    Ok(doc)
}

/// Parses a RunProfile document from JSON text.
pub fn parse_run_profile_json(input: &str) -> Result<RunProfileDocument, SchemaError> {
    let doc: RunProfileDocument =
        serde_json::from_str(input).map_err(|error| SchemaError::Parse(error.to_string()))?;
    ensure_kind(doc.kind, DocumentKind::RunProfile)?;
    ensure_version(doc.schema_version)?;
    Ok(doc)
}

fn ensure_kind(actual: DocumentKind, expected: DocumentKind) -> Result<(), SchemaError> {
    if actual != expected {
        return Err(SchemaError::WrongDocumentKind(actual));
    }
    Ok(())
}

fn ensure_version(version: u32) -> Result<(), SchemaError> {
    if version != CURRENT_SCHEMA_VERSION {
        return Err(SchemaError::UnsupportedVersion(version));
    }
    Ok(())
}

/// Shape-validation failure that prevents a reliable diagnostic result.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SchemaError {
    /// Declared schema version is unsupported.
    UnsupportedVersion(u32),
    /// Declared document kind conflicts with the target type.
    WrongDocumentKind(DocumentKind),
    /// The document text could not be decoded.
    Parse(String),
}

impl std::fmt::Display for SchemaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedVersion(version) => {
                write!(f, "unsupported schema_version: {version}")
            }
            Self::WrongDocumentKind(kind) => write!(f, "wrong document kind: {kind:?}"),
            Self::Parse(message) => write!(f, "parse error: {message}"),
        }
    }
}

impl std::error::Error for SchemaError {}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;
    use crate::document::{DatasetBinding, ExecutionSpec, Metadata};
    use crate::model::meteorology::DatasetRef;
    use crate::model::population::{DomainFillAirMassSpec, PopulationId};
    use crate::quantity::{Mass, QuantityInput, UnitRegistry};
    use std::path::PathBuf;

    #[test]
    fn parses_and_validates_minimal_case() {
        let yaml = r#"
schema_version: 0
kind: case
metadata:
  name: probe
time:
  start:
    seconds_since_unix_epoch: 0
    nanosecond: 0
  end:
    seconds_since_unix_epoch: 3600
    nanosecond: 0
  direction: forward
meteorology:
  domains:
    - id: d0
      dataset: era5
      priority: 10
      horizontal_halo_cells: 1
"#;
        let doc = parse_case_yaml(yaml).unwrap();
        let bag = doc.validate_shape().unwrap();
        assert!(!bag.has_errors(), "{:?}", bag.sorted());
    }

    #[test]
    fn rejects_unknown_top_level_and_nested_fields() {
        let yaml = r#"
schema_version: 0
kind: case
metadata:
  name: probe
typo_field: 1
"#;
        assert!(parse_case_yaml(yaml).is_err());

        let yaml = r#"
schema_version: 0
kind: case
metadata:
  name: probe
  unknown_meta: true
"#;
        assert!(parse_case_yaml(yaml).is_err());
    }

    #[test]
    fn domain_fill_count_zero_and_both_targets_fail() {
        let mass = UnitRegistry::standard()
            .resolve::<Mass>(&QuantityInput::object(1.0, "kg"))
            .unwrap();
        let mut doc = CaseDocument {
            schema_version: 0,
            kind: DocumentKind::Case,
            metadata: Metadata {
                name: "p".into(),
                ..Metadata::default()
            },
            time: None,
            meteorology: None,
            particle_population: Some(ComponentRef::Inline(
                ParticlePopulationSpec::DomainFillAirMass(DomainFillAirMassSpec {
                    id: PopulationId("p0".into()),
                    target_particle_mass: None,
                    target_particle_count: Some(0),
                }),
            )),
            substances: None,
            numerics: None,
            physics: None,
            outputs: None,
        };
        let bag = doc.validate_shape().unwrap();
        assert!(
            bag.iter()
                .any(|d| d.code() == "case.population.target_count_zero")
        );

        doc.particle_population = Some(ComponentRef::Inline(
            ParticlePopulationSpec::DomainFillAirMass(DomainFillAirMassSpec {
                id: PopulationId("p0".into()),
                target_particle_mass: Some(mass),
                target_particle_count: Some(1),
            }),
        ));
        let bag = doc.validate_shape().unwrap();
        assert!(
            bag.iter()
                .any(|d| d.code() == "case.population.target_both")
        );
    }

    #[test]
    fn domain_parent_missing_and_cycle() {
        let yaml = r#"
schema_version: 0
kind: case
metadata: { name: d }
meteorology:
  domains:
    - id: a
      dataset: era5
      priority: 1
      parent: b
      horizontal_halo_cells: 1
    - id: b
      dataset: era5
      priority: 2
      parent: a
      horizontal_halo_cells: 1
"#;
        let doc = parse_case_yaml(yaml).unwrap();
        let bag = doc.validate_shape().unwrap();
        assert!(
            bag.iter()
                .any(|d| d.code() == "case.meteorology.parent_cycle")
        );
    }

    #[test]
    fn run_profile_duplicate_dataset_binding() {
        let doc = RunProfileDocument {
            schema_version: 0,
            kind: DocumentKind::RunProfile,
            metadata: Metadata {
                name: "r".into(),
                ..Metadata::default()
            },
            case_path: PathBuf::from("case.yaml"),
            datasets: vec![
                DatasetBinding {
                    dataset: DatasetRef("era5".into()),
                    lockfile: PathBuf::from("a.lock"),
                    cache_root: None,
                },
                DatasetBinding {
                    dataset: DatasetRef("era5".into()),
                    lockfile: PathBuf::from("b.lock"),
                    cache_root: None,
                },
            ],
            execution: ExecutionSpec {
                worker_threads: 1,
                memory_budget_bytes: 1,
                executor: "cpu".into(),
            },
        };
        let bag = doc.validate_shape().unwrap();
        assert!(
            bag.iter()
                .any(|d| d.code() == "run_profile.dataset_duplicate")
        );
    }
}
