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
use std::path::Component;

use crate::diagnostic::{Diagnostic, DiagnosticBag, DiagnosticPath};
use crate::document::{
    CaseDocument, DataRootId, DocumentKind, ExecutionSpec, ResolvedCase, RunProfileDocument,
};
use crate::model::meteorology::{DomainId, MeteorologySpec};
use crate::model::numerics::NumericsSpec;
use crate::model::output::{OutputProductSpec, OutputSchedule};
use crate::model::physics::{
    PhysicsModuleId, PhysicsPreset, PhysicsSelectionSpec, resolve_physics,
    validate_resolved_physics,
};
use crate::model::population::{
    DomainFillAirMassSpec, GeoJsonGeometry, GeoJsonSource, ParticlePopulationSpec,
    ReleaseEventSpec, ReleaseVerticalSpec,
};
use crate::model::substance::{SubstanceKind, SubstanceSpec};
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
            validate_physics_selection(
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
    if let Some(physics) = &case.physics {
        diagnostics.append(validate_resolved_physics(
            physics,
            DiagnosticPath::root().field("physics"),
        ));
    }
    validate_outputs(
        &case.outputs,
        DiagnosticPath::root().field("outputs"),
        &mut diagnostics,
    );
    validate_cross_component_contracts(case, &mut diagnostics);
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
        if self.output_root.as_os_str().is_empty() {
            diagnostics.push(
                Diagnostic::error(
                    "run_profile.output_root_empty",
                    "output_root must not be empty",
                )
                .at(DiagnosticPath::root().field("output_root")),
            );
        }
        validate_execution(
            &self.execution,
            DiagnosticPath::root().field("execution"),
            &mut diagnostics,
        );
        let mut seen_datasets = BTreeSet::new();
        for (index, binding) in self.datasets.iter().enumerate() {
            let binding_path = DiagnosticPath::root().field("datasets").index(index);
            if binding.dataset.0.trim().is_empty() {
                diagnostics.push(
                    Diagnostic::error(
                        "run_profile.dataset_empty",
                        "dataset identifier must not be empty",
                    )
                    .at(binding_path.clone().field("dataset")),
                );
            } else if !seen_datasets.insert(binding.dataset.0.clone()) {
                diagnostics.push(
                    Diagnostic::error(
                        "run_profile.dataset_duplicate",
                        format!("duplicate dataset binding '{}'", binding.dataset.0),
                    )
                    .at(binding_path.clone().field("dataset")),
                );
            }
            if binding.lockfile.as_os_str().is_empty() {
                diagnostics.push(
                    Diagnostic::error(
                        "run_profile.lockfile_empty",
                        "dataset lockfile path must not be empty",
                    )
                    .at(binding_path.clone().field("lockfile")),
                );
            }
            for (root_id, path) in &binding.data_roots {
                if root_id.0.trim().is_empty() {
                    diagnostics.push(
                        Diagnostic::error(
                            "run_profile.data_root_id_empty",
                            "dataset data-root identifier must not be empty",
                        )
                        .at(binding_path.clone().field("data_roots")),
                    );
                } else if root_id.0 == DataRootId::LOCKFILE {
                    diagnostics.push(
                        Diagnostic::error(
                            "run_profile.data_root_reserved",
                            "data-root identifier 'lockfile' is reserved",
                        )
                        .at(binding_path.clone().field("data_roots")),
                    );
                }
                if path.as_os_str().is_empty() {
                    diagnostics.push(
                        Diagnostic::error(
                            "run_profile.data_root_path_empty",
                            "dataset data-root path must not be empty",
                        )
                        .at(binding_path.clone().field("data_roots")),
                    );
                }
            }
        }

        let mut seen_profile_sources = BTreeSet::new();
        for (index, source) in self.profile_sources.iter().enumerate() {
            let path = source.path();
            let source_path = DiagnosticPath::root().field("profile_sources").index(index);
            if path.as_os_str().is_empty() {
                diagnostics.push(
                    Diagnostic::error(
                        "run_profile.profile_source_empty",
                        "Profile source path must not be empty",
                    )
                    .at(source_path.clone().field("path")),
                );
            } else if !seen_profile_sources.insert(path.clone()) {
                diagnostics.push(
                    Diagnostic::error(
                        "run_profile.profile_source_duplicate",
                        format!("duplicate Profile source '{}'", path.display()),
                    )
                    .at(source_path.field("path")),
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
    let mut policies = BTreeSet::new();
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
        } else if !policies.insert(policy.0.as_str()) {
            diagnostics.push(
                Diagnostic::error(
                    "case.numerics.boundary_duplicate",
                    format!("duplicate boundary policy '{}'", policy.0),
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
            if spec.events.is_empty() {
                diagnostics.push(
                    Diagnostic::error(
                        "case.population.events_empty",
                        "release-driven population must contain at least one event",
                    )
                    .at(path.clone().field("events")),
                );
            }
            let mut ids = BTreeSet::new();
            for (index, event) in spec.events.iter().enumerate() {
                let event_path = path.clone().field("events").index(index);
                if event.id.0.trim().is_empty() {
                    diagnostics.push(
                        Diagnostic::error(
                            "case.release.event_id_empty",
                            "release event id must not be empty",
                        )
                        .at(event_path.clone().field("id")),
                    );
                } else if !ids.insert(event.id.0.clone()) {
                    diagnostics.push(
                        Diagnostic::error(
                            "case.release.event_id_duplicate",
                            format!("duplicate release event id '{}'", event.id.0),
                        )
                        .at(event_path.clone().field("id")),
                    );
                }
                validate_release_event(event, event_path, diagnostics);
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
                    .at(path.clone().field("ozone_rule")),
                );
            }
            if spec.ozone_substance.0.trim().is_empty() {
                diagnostics.push(
                    Diagnostic::error(
                        "case.population.ozone_substance_empty",
                        "ozone_substance must not be empty",
                    )
                    .at(path.field("ozone_substance")),
                );
            }
        }
    }
}

fn validate_release_event(
    event: &ReleaseEventSpec,
    path: DiagnosticPath,
    diagnostics: &mut DiagnosticBag,
) {
    if event.start > event.end {
        diagnostics.push(
            Diagnostic::error(
                "case.release.time_inverted",
                "release event requires start <= end in physical time",
            )
            .at(path.clone()),
        );
    }
    if event.particle_count == 0 {
        diagnostics.push(
            Diagnostic::error(
                "case.release.particle_count_zero",
                "release event particle_count must be > 0",
            )
            .at(path.clone().field("particle_count")),
        );
    }
    if event.mass.is_empty() {
        diagnostics.push(
            Diagnostic::error(
                "case.release.mass_empty",
                "release event must define at least one substance mass",
            )
            .at(path.clone().field("mass")),
        );
    } else {
        let mut any_positive = false;
        for (substance, mass) in &event.mass {
            if substance.0.trim().is_empty() {
                diagnostics.push(
                    Diagnostic::error(
                        "case.release.mass_substance_empty",
                        "release mass substance id must not be empty",
                    )
                    .at(path.clone().field("mass")),
                );
            }
            let value = mass.value_si();
            if !value.is_finite() || value < 0.0 {
                diagnostics.push(
                    Diagnostic::error(
                        "case.release.mass_invalid",
                        "release mass must be finite and non-negative",
                    )
                    .at(path.clone().field("mass")),
                );
            }
            any_positive |= value > 0.0;
        }
        if !any_positive {
            diagnostics.push(
                Diagnostic::error(
                    "case.release.mass_all_zero",
                    "release event total mass must be greater than zero",
                )
                .at(path.clone().field("mass")),
            );
        }
    }
    validate_geojson_source(&event.geometry, path.clone().field("geometry"), diagnostics);
    validate_release_vertical(&event.vertical, path.field("vertical"), diagnostics);
}

fn validate_geojson_source(
    source: &GeoJsonSource,
    path: DiagnosticPath,
    diagnostics: &mut DiagnosticBag,
) {
    match source {
        GeoJsonSource::Inline { geometry } => {
            validate_geojson_geometry(geometry, path.field("geometry"), diagnostics);
        }
        GeoJsonSource::File { path: file_path } => {
            let text = file_path.to_string_lossy();
            if file_path.as_os_str().is_empty()
                || file_path.is_absolute()
                || file_path
                    .components()
                    .any(|component| matches!(component, Component::ParentDir))
                || text.contains("://")
                || file_path
                    .extension()
                    .and_then(|extension| extension.to_str())
                    .is_none_or(|extension| !extension.eq_ignore_ascii_case("geojson"))
            {
                diagnostics.push(
                    Diagnostic::error(
                        "case.release.geometry_path_unsafe",
                        "GeoJSON path must be a non-empty local relative path without '..'",
                    )
                    .at(path.field("path")),
                );
            }
        }
    }
}

fn validate_geojson_geometry(
    geometry: &GeoJsonGeometry,
    path: DiagnosticPath,
    diagnostics: &mut DiagnosticBag,
) {
    let invalid = match geometry {
        GeoJsonGeometry::Point(point) => coordinate_is_invalid(point),
        GeoJsonGeometry::MultiPoint(points) => {
            points.is_empty() || points.iter().any(coordinate_is_invalid)
        }
        GeoJsonGeometry::LineString(line) => {
            line.len() < 2 || line.iter().any(coordinate_is_invalid)
        }
        GeoJsonGeometry::MultiLineString(lines) => {
            lines.is_empty()
                || lines.iter().any(|line| line.len() < 2)
                || lines
                    .iter()
                    .flat_map(|line| line.iter())
                    .any(coordinate_is_invalid)
        }
        GeoJsonGeometry::Polygon(rings) => {
            !rings_are_structurally_valid(rings)
                || rings
                    .iter()
                    .flat_map(|ring| ring.iter())
                    .any(coordinate_is_invalid)
        }
        GeoJsonGeometry::MultiPolygon(polygons) => {
            polygons.is_empty()
                || polygons
                    .iter()
                    .any(|rings| !rings_are_structurally_valid(rings))
                || polygons
                    .iter()
                    .flat_map(|rings| rings.iter())
                    .flat_map(|ring| ring.iter())
                    .any(coordinate_is_invalid)
        }
    };
    if invalid {
        diagnostics.push(
            Diagnostic::error(
                "case.release.geometry_shape_invalid",
                "GeoJSON geometry is empty, non-finite, out of latitude range, or structurally degenerate",
            )
            .at(path),
        );
    }
}

fn coordinate_is_invalid(coordinate: &[f64; 2]) -> bool {
    !coordinate[0].is_finite()
        || !coordinate[1].is_finite()
        || !(-90.0..=90.0).contains(&coordinate[1])
}

fn rings_are_structurally_valid(rings: &[Vec<[f64; 2]>]) -> bool {
    !rings.is_empty()
        && rings.iter().all(|ring| {
            ring.len() >= 4
                && ring
                    .first()
                    .zip(ring.last())
                    .is_some_and(|(first, last)| first == last)
        })
}

fn validate_release_vertical(
    vertical: &ReleaseVerticalSpec,
    path: DiagnosticPath,
    diagnostics: &mut DiagnosticBag,
) {
    let (lower, upper, minimum, strict_minimum, code) = match vertical {
        ReleaseVerticalSpec::AboveSeaLevel { lower, upper } => (
            lower.value_si(),
            upper.as_ref().map(Quantity::value_si),
            f64::NEG_INFINITY,
            false,
            "asl",
        ),
        ReleaseVerticalSpec::AboveGround { lower, upper } => (
            lower.value_si(),
            upper.as_ref().map(Quantity::value_si),
            0.0,
            false,
            "agl",
        ),
        ReleaseVerticalSpec::Pressure { lower, upper } => (
            lower.value_si(),
            upper.as_ref().map(Quantity::value_si),
            0.0,
            true,
            "pressure",
        ),
    };
    let below_minimum = if strict_minimum {
        lower <= minimum
    } else {
        lower < minimum
    };
    let invalid_lower = !lower.is_finite() || below_minimum;
    let invalid_upper = upper.is_some_and(|value| {
        let below_minimum = if strict_minimum {
            value <= minimum
        } else {
            value < minimum
        };
        !value.is_finite() || value < lower || below_minimum
    });
    if invalid_lower || invalid_upper {
        diagnostics.push(
            Diagnostic::error(
                "case.release.vertical_invalid",
                format!("{code} release bounds are invalid or not ordered"),
            )
            .at(path),
        );
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
    if spec.domain_id.0.trim().is_empty() {
        diagnostics.push(
            Diagnostic::error(
                "case.population.domain_id_empty",
                "domain-fill domain_id must not be empty",
            )
            .at(path.clone().field("domain_id")),
        );
    }
    match (
        &spec.target_dry_air_mass_per_particle,
        &spec.target_particle_count,
    ) {
        (None, None) => {
            diagnostics.push(
                Diagnostic::error(
                    "case.population.target_missing",
                    "domain-fill requires exactly one of target_dry_air_mass_per_particle or target_particle_count",
                )
                .at(path),
            );
        }
        (Some(_), Some(_)) => {
            diagnostics.push(
                Diagnostic::error(
                    "case.population.target_both",
                    "domain-fill must not set both target_dry_air_mass_per_particle and target_particle_count",
                )
                .at(path),
            );
        }
        (Some(mass), None) => {
            if !mass.is_positive_finite() {
                diagnostics.push(
                    Diagnostic::error(
                        "case.population.target_mass_invalid",
                        "target_dry_air_mass_per_particle must be a finite positive mass",
                    )
                    .at(path.field("target_dry_air_mass_per_particle")),
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
        let item_path = path.clone().index(index);
        if substance.id().0.trim().is_empty() {
            diagnostics.push(
                Diagnostic::error("case.substance.id_empty", "substance id must not be empty")
                    .at(item_path.clone().field("id")),
            );
        } else if !seen.insert(substance.id().0.clone()) {
            diagnostics.push(
                Diagnostic::error(
                    "case.substance.id_duplicate",
                    format!("duplicate substance id '{}'", substance.id().0),
                )
                .at(item_path.clone().field("id")),
            );
        }
        if substance.display_name().trim().is_empty() {
            diagnostics.push(
                Diagnostic::error(
                    "case.substance.display_name_empty",
                    "substance display_name must not be empty",
                )
                .at(item_path.clone().field("display_name")),
            );
        }
        match substance {
            SubstanceSpec::WaterVapor { .. } => {}
            SubstanceSpec::Gas {
                molar_mass,
                henry_constant,
                surface_reactivity,
                half_life,
                oh_reaction,
                ..
            } => {
                if molar_mass.value_si() <= 0.0 {
                    diagnostics.push(
                        Diagnostic::error(
                            "case.substance.molar_mass_invalid",
                            "molar_mass must be finite and positive",
                        )
                        .at(item_path.clone().field("molar_mass")),
                    );
                }
                if henry_constant.value_si() < 0.0 {
                    diagnostics.push(
                        Diagnostic::error(
                            "case.substance.henry_constant_invalid",
                            "henry_constant must be finite and non-negative",
                        )
                        .at(item_path.clone().field("henry_constant")),
                    );
                }
                if !surface_reactivity.is_finite() || !(0.0..=1.0).contains(surface_reactivity) {
                    diagnostics.push(
                        Diagnostic::error(
                            "case.substance.surface_reactivity_invalid",
                            "surface_reactivity must be finite and within [0, 1]",
                        )
                        .at(item_path.clone().field("surface_reactivity")),
                    );
                }
                if half_life
                    .as_ref()
                    .is_some_and(|value| !value.is_positive_finite())
                {
                    diagnostics.push(
                        Diagnostic::error(
                            "case.substance.half_life_invalid",
                            "half_life must be finite and positive",
                        )
                        .at(item_path.clone().field("half_life")),
                    );
                }
                if let Some(reaction) = oh_reaction {
                    if reaction.pre_exponential.value_si() < 0.0 {
                        diagnostics.push(
                            Diagnostic::error(
                                "case.substance.oh_pre_exponential_invalid",
                                "OH pre_exponential must be finite and non-negative",
                            )
                            .at(item_path
                                .clone()
                                .field("oh_reaction")
                                .field("pre_exponential")),
                        );
                    }
                    if !reaction.temperature_exponent.is_finite() {
                        diagnostics.push(
                            Diagnostic::error(
                                "case.substance.oh_temperature_exponent_invalid",
                                "OH temperature_exponent must be finite",
                            )
                            .at(item_path
                                .clone()
                                .field("oh_reaction")
                                .field("temperature_exponent")),
                        );
                    }
                }
            }
            SubstanceSpec::Aerosol {
                material_density,
                diameter,
                ..
            } => {
                if material_density.value_si() <= 0.0 {
                    diagnostics.push(
                        Diagnostic::error(
                            "case.substance.material_density_invalid",
                            "material_density must be finite and positive",
                        )
                        .at(item_path.clone().field("material_density")),
                    );
                }
                let minimum = diameter.minimum.value_si();
                let mean = diameter.geometric_mean.value_si();
                let maximum = diameter.maximum.value_si();
                if minimum < 1.0e-8 || maximum > 1.0e-4 || minimum > mean || mean > maximum {
                    diagnostics.push(
                        Diagnostic::error(
                            "case.substance.diameter_range_invalid",
                            "diameter must satisfy 0.01 um <= minimum <= geometric_mean <= maximum <= 100 um",
                        )
                        .at(item_path.clone().field("diameter")),
                    );
                }
                if !diameter.geometric_standard_deviation.is_finite()
                    || diameter.geometric_standard_deviation <= 1.0
                {
                    diagnostics.push(
                        Diagnostic::error(
                            "case.substance.diameter_spread_invalid",
                            "geometric_standard_deviation must be finite and greater than 1",
                        )
                        .at(item_path
                            .clone()
                            .field("diameter")
                            .field("geometric_standard_deviation")),
                    );
                }
            }
        }
    }
}

fn validate_physics_selection(
    physics: &PhysicsSelectionSpec,
    path: DiagnosticPath,
    diagnostics: &mut DiagnosticBag,
) {
    if let Err(physics_diagnostics) = resolve_physics(physics, path) {
        diagnostics.append(physics_diagnostics);
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
        if product.sink.model.0.trim().is_empty() {
            diagnostics.push(
                Diagnostic::error(
                    "case.output.sink_empty",
                    "output sink model id must not be empty",
                )
                .at(product_path.clone().field("sink").field("model")),
            );
        }
        if let OutputSchedule::Interval { interval, .. } = &product.schedule {
            validate_positive_duration(
                interval,
                product_path.field("schedule").field("interval"),
                "case.output.interval_invalid",
                "output interval must be a finite positive duration",
                diagnostics,
            );
        }
    }
}

fn validate_cross_component_contracts(case: &ResolvedCase, diagnostics: &mut DiagnosticBag) {
    let domain_ids = case
        .meteorology
        .as_ref()
        .map(|met| {
            met.domains
                .iter()
                .map(|domain| domain.id.0.as_str())
                .collect::<BTreeSet<_>>()
        })
        .unwrap_or_default();
    let substance_ids = case
        .substances
        .iter()
        .map(|substance| substance.id().0.as_str())
        .collect::<BTreeSet<_>>();

    validate_physics_substances(case, diagnostics);

    let Some(population) = &case.particle_population else {
        return;
    };
    match population {
        ParticlePopulationSpec::ReleaseDriven(spec) => {
            let physical_bounds = case.time.as_ref().map(|time| {
                if time.start <= time.end {
                    (time.start, time.end)
                } else {
                    (time.end, time.start)
                }
            });
            for (index, event) in spec.events.iter().enumerate() {
                let path = DiagnosticPath::root()
                    .field("particle_population")
                    .field("events")
                    .index(index);
                if let Some((start, end)) = physical_bounds {
                    if event.start < start || event.end > end {
                        diagnostics.push(
                            Diagnostic::error(
                                "case.release.outside_simulation_time",
                                "release event must lie completely inside the simulation time range",
                            )
                            .at(path.clone()),
                        );
                    }
                }
                for substance in event.mass.keys() {
                    if !substance_ids.contains(substance.0.as_str()) {
                        diagnostics.push(
                            Diagnostic::error(
                                "case.release.substance_unknown",
                                format!("release references unknown substance '{}'", substance.0),
                            )
                            .at(path.clone().field("mass")),
                        );
                    }
                }
            }
        }
        ParticlePopulationSpec::DomainFillAirMass(spec) => {
            validate_population_domain_reference(
                spec,
                DiagnosticPath::root().field("particle_population"),
                &domain_ids,
                diagnostics,
            );
        }
        ParticlePopulationSpec::DomainFillStratosphericOzone(spec) => {
            validate_population_domain_reference(
                &spec.air_mass,
                DiagnosticPath::root()
                    .field("particle_population")
                    .field("air_mass"),
                &domain_ids,
                diagnostics,
            );
            if !substance_ids.contains(spec.ozone_substance.0.as_str()) {
                diagnostics.push(
                    Diagnostic::error(
                        "case.population.ozone_substance_unknown",
                        format!(
                            "ozone_substance '{}' is not declared in substances",
                            spec.ozone_substance.0
                        ),
                    )
                    .at(DiagnosticPath::root()
                        .field("particle_population")
                        .field("ozone_substance")),
                );
            }
        }
    }
}

fn validate_physics_substances(case: &ResolvedCase, diagnostics: &mut DiagnosticBag) {
    let Some(physics) = &case.physics else {
        return;
    };
    let allowed = match physics.preset {
        PhysicsPreset::WaterVaporTracking => &[SubstanceKind::WaterVapor][..],
        PhysicsPreset::GasTransport => &[SubstanceKind::Gas][..],
        PhysicsPreset::AerosolTransport => &[SubstanceKind::Aerosol][..],
        PhysicsPreset::BuoyantRelease => &[
            SubstanceKind::WaterVapor,
            SubstanceKind::Gas,
            SubstanceKind::Aerosol,
        ][..],
    };
    if case.substances.is_empty() {
        diagnostics.push(
            Diagnostic::error(
                "case.physics.substance_missing",
                "an enabled physics preset requires at least one substance",
            )
            .at(DiagnosticPath::root().field("substances")),
        );
        return;
    }
    for (index, substance) in case.substances.iter().enumerate() {
        if !allowed.contains(&substance.kind()) {
            diagnostics.push(
                Diagnostic::error(
                    "case.physics.substance_kind_mismatch",
                    format!(
                        "substance '{}' is incompatible with preset {:?}",
                        substance.id().0,
                        physics.preset
                    ),
                )
                .at(DiagnosticPath::root()
                    .field("substances")
                    .index(index)
                    .field("kind")),
            );
        }
    }

    for (index, module) in physics.modules.iter().enumerate() {
        let supports = |kind| match module.model {
            PhysicsModuleId::WaterVaporExchange => kind == SubstanceKind::WaterVapor,
            PhysicsModuleId::GravitationalSettling => kind == SubstanceKind::Aerosol,
            PhysicsModuleId::DryDeposition | PhysicsModuleId::WetScavenging => {
                matches!(kind, SubstanceKind::Gas | SubstanceKind::Aerosol)
            }
            PhysicsModuleId::FirstOrderDecay | PhysicsModuleId::OhOxidation => {
                kind == SubstanceKind::Gas
            }
            _ => true,
        };
        if !case
            .substances
            .iter()
            .any(|substance| supports(substance.kind()))
        {
            diagnostics.push(
                Diagnostic::error(
                    "case.physics.module_has_no_substance",
                    format!("module '{}' has no compatible substance", module.model),
                )
                .at(DiagnosticPath::root()
                    .field("physics")
                    .field("modules")
                    .index(index)
                    .field("model")),
            );
        }
        let configured = match module.model {
            PhysicsModuleId::FirstOrderDecay => case.substances.iter().any(|substance| {
                matches!(
                    substance,
                    SubstanceSpec::Gas {
                        half_life: Some(_),
                        ..
                    }
                )
            }),
            PhysicsModuleId::OhOxidation => case.substances.iter().any(|substance| {
                matches!(
                    substance,
                    SubstanceSpec::Gas {
                        oh_reaction: Some(_),
                        ..
                    }
                )
            }),
            _ => true,
        };
        if !configured {
            diagnostics.push(
                Diagnostic::error(
                    "case.physics.module_property_missing",
                    format!(
                        "module '{}' has no substance declaring its required property",
                        module.model
                    ),
                )
                .at(DiagnosticPath::root()
                    .field("physics")
                    .field("modules")
                    .index(index)
                    .field("model")),
            );
        }
    }
}

fn validate_population_domain_reference(
    spec: &DomainFillAirMassSpec,
    path: DiagnosticPath,
    domain_ids: &BTreeSet<&str>,
    diagnostics: &mut DiagnosticBag,
) {
    if !domain_ids.contains(spec.domain_id.0.as_str()) {
        diagnostics.push(
            Diagnostic::error(
                "case.population.domain_unknown",
                format!(
                    "domain-fill references unknown domain '{}'",
                    spec.domain_id.0
                ),
            )
            .at(path.field("domain_id")),
        );
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
                    domain_id: DomainId("d0".into()),
                    target_dry_air_mass_per_particle: None,
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
                domain_id: DomainId("d0".into()),
                target_dry_air_mass_per_particle: Some(mass),
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
            output_root: PathBuf::from("output"),
            datasets: vec![
                DatasetBinding {
                    dataset: DatasetRef("era5".into()),
                    lockfile: PathBuf::from("a.lock"),
                    cache_root: None,
                    data_roots: BTreeMap::new(),
                    reader_backend: None,
                },
                DatasetBinding {
                    dataset: DatasetRef("era5".into()),
                    lockfile: PathBuf::from("b.lock"),
                    cache_root: None,
                    data_roots: BTreeMap::new(),
                    reader_backend: None,
                },
            ],
            profile_sources: vec![],
            execution: ExecutionSpec {
                worker_threads: 1,
                memory_budget_bytes: 1,
                executor: "cpu".into(),
                meteorology_reader: Default::default(),
            },
        };
        let bag = doc.validate_shape().unwrap();
        assert!(
            bag.iter()
                .any(|d| d.code() == "run_profile.dataset_duplicate")
        );
    }
}
