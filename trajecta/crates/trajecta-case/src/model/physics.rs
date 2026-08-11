//! # Contract: M6 physics selection and resolution
//!
//! A Case selects one frozen preset, removes or overrides named modules, then
//! appends explicitly configured modules. Resolution produces one ordered,
//! complete module list. The former `enabled` flag and string parameter map are
//! intentionally absent from this contract.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::diagnostic::{Diagnostic, DiagnosticBag, DiagnosticPath};
use crate::quantity::{Quantity, Time, Unit};

/// Stable identifier for an algorithm or pluggable model outside M6 physics.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct ModelId(pub String);

/// Closed set of M6 physical-process modules.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PhysicsModuleId {
    /// Calendar and explicit-series emission scheduling.
    EmissionTimeProfile,
    /// Entraining one-dimensional buoyant plume.
    BuoyantPlumeRise,
    /// Sub-grid terrain displacement and mixing enhancement.
    SubgridOrography,
    /// Thomson/Hanna boundary-layer Langevin motion.
    BoundaryLayerLangevin,
    /// Three-dimensional mesoscale Markov velocity.
    MesoscaleMarkov,
    /// Aerosol gravitational settling.
    GravitationalSettling,
    /// Conservative deep-convection column transfer.
    DeepConvectionColumn,
    /// Conservative water-vapour exchange.
    WaterVaporExchange,
    /// Gas and aerosol dry deposition.
    DryDeposition,
    /// Phase-resolved wet scavenging.
    WetScavenging,
    /// Exact exponential half-life loss.
    FirstOrderDecay,
    /// Temperature-dependent OH oxidation.
    OhOxidation,
}

impl PhysicsModuleId {
    /// Returns the stable snake-case wire identifier.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::EmissionTimeProfile => "emission_time_profile",
            Self::BuoyantPlumeRise => "buoyant_plume_rise",
            Self::SubgridOrography => "subgrid_orography",
            Self::BoundaryLayerLangevin => "boundary_layer_langevin",
            Self::MesoscaleMarkov => "mesoscale_markov",
            Self::GravitationalSettling => "gravitational_settling",
            Self::DeepConvectionColumn => "deep_convection_column",
            Self::WaterVaporExchange => "water_vapor_exchange",
            Self::DryDeposition => "dry_deposition",
            Self::WetScavenging => "wet_scavenging",
            Self::FirstOrderDecay => "first_order_decay",
            Self::OhOxidation => "oh_oxidation",
        }
    }

    /// Stage in which the production executor becomes available.
    #[must_use]
    pub const fn implementation_stage(self) -> &'static str {
        match self {
            Self::BoundaryLayerLangevin => "m6_a1",
            Self::SubgridOrography | Self::MesoscaleMarkov => "m6_a2",
            Self::DeepConvectionColumn => "m6_a3",
            Self::WaterVaporExchange => "m6_a4",
            Self::GravitationalSettling | Self::DryDeposition => "m6_a5",
            Self::WetScavenging | Self::FirstOrderDecay | Self::OhOxidation => "m6_a6",
            Self::EmissionTimeProfile | Self::BuoyantPlumeRise => "m6_a7",
        }
    }

    const fn default_maximum_substep_seconds(self) -> Option<f64> {
        match self {
            Self::EmissionTimeProfile | Self::FirstOrderDecay | Self::OhOxidation => None,
            Self::BuoyantPlumeRise | Self::BoundaryLayerLangevin => Some(30.0),
            Self::SubgridOrography
            | Self::MesoscaleMarkov
            | Self::DeepConvectionColumn
            | Self::WaterVaporExchange => Some(300.0),
            Self::GravitationalSettling | Self::DryDeposition | Self::WetScavenging => Some(60.0),
        }
    }

    const fn maximum_substep_ceiling_seconds(self) -> Option<f64> {
        match self {
            Self::EmissionTimeProfile | Self::FirstOrderDecay | Self::OhOxidation => None,
            Self::BoundaryLayerLangevin => Some(120.0),
            Self::BuoyantPlumeRise
            | Self::GravitationalSettling
            | Self::DryDeposition
            | Self::WetScavenging => Some(300.0),
            Self::SubgridOrography
            | Self::MesoscaleMarkov
            | Self::DeepConvectionColumn
            | Self::WaterVaporExchange => Some(900.0),
        }
    }

    const fn requires(self) -> &'static [Self] {
        match self {
            Self::BuoyantPlumeRise => &[Self::EmissionTimeProfile],
            _ => &[],
        }
    }
}

impl std::fmt::Display for PhysicsModuleId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Frozen task-level physics preset.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PhysicsPreset {
    /// Moisture-source and sink attribution.
    WaterVaporTracking,
    /// Gas transport, deposition, and first-order loss.
    GasTransport,
    /// Size-resolved aerosol transport and removal.
    AerosolTransport,
    /// Time-varying buoyant release followed by atmospheric transport.
    BuoyantRelease,
}

impl PhysicsPreset {
    /// Returns the preset module sequence before removals and overrides.
    #[must_use]
    pub const fn modules(self) -> &'static [PhysicsModuleId] {
        use PhysicsModuleId as M;
        match self {
            Self::WaterVaporTracking => &[
                M::SubgridOrography,
                M::BoundaryLayerLangevin,
                M::MesoscaleMarkov,
                M::DeepConvectionColumn,
                M::WaterVaporExchange,
            ],
            Self::GasTransport => &[
                M::SubgridOrography,
                M::BoundaryLayerLangevin,
                M::MesoscaleMarkov,
                M::DeepConvectionColumn,
                M::DryDeposition,
                M::WetScavenging,
                M::FirstOrderDecay,
                M::OhOxidation,
            ],
            Self::AerosolTransport => &[
                M::SubgridOrography,
                M::BoundaryLayerLangevin,
                M::MesoscaleMarkov,
                M::GravitationalSettling,
                M::DeepConvectionColumn,
                M::DryDeposition,
                M::WetScavenging,
            ],
            Self::BuoyantRelease => &[
                M::EmissionTimeProfile,
                M::BuoyantPlumeRise,
                M::SubgridOrography,
                M::BoundaryLayerLangevin,
                M::MesoscaleMarkov,
                M::DeepConvectionColumn,
            ],
        }
    }
}

/// Sparse module declaration used by `overrides` and appended `modules`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PhysicsModuleConfig {
    /// Selected physical-process module.
    pub model: PhysicsModuleId,
    /// Optional explicit final order.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub order: Option<usize>,
    /// Optional module stability ceiling.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub maximum_substep: Option<Quantity<Time>>,
    /// Fraction of the native meteorology interval used as the mesoscale
    /// velocity correlation time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub correlation_interval_fraction: Option<f64>,
}

/// User-authored preset selection and patches.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PhysicsSelectionSpec {
    /// Base task preset.
    pub preset: PhysicsPreset,
    /// Modules removed from the selected preset.
    #[serde(default)]
    pub remove: Vec<PhysicsModuleId>,
    /// Sparse patches applied to modules that remain after removal.
    #[serde(default)]
    pub overrides: Vec<PhysicsModuleConfig>,
    /// Modules appended after preset patching.
    #[serde(default)]
    pub modules: Vec<PhysicsModuleConfig>,
}

/// One complete module in execution order.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResolvedPhysicsModule {
    /// Selected module.
    pub model: PhysicsModuleId,
    /// Unique contiguous execution order.
    pub order: usize,
    /// Normalized stability ceiling in seconds when the module owns one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub maximum_substep: Option<Quantity<Time>>,
    /// Mesoscale OU correlation time divided by the native data interval.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub correlation_interval_fraction: Option<f64>,
}

/// Fully expanded physics selection stored in a resolved Case and provenance.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResolvedPhysicsSpec {
    /// Original base preset.
    pub preset: PhysicsPreset,
    /// Explicit removals in user order.
    pub remove: Vec<PhysicsModuleId>,
    /// Explicit sparse overrides with durations normalized to seconds.
    pub overrides: Vec<PhysicsModuleConfig>,
    /// Complete final module sequence.
    pub modules: Vec<ResolvedPhysicsModule>,
}

/// Expands and validates a physics selection using the frozen M6 merge order.
pub fn resolve_physics(
    selection: &PhysicsSelectionSpec,
    path: DiagnosticPath,
) -> Result<ResolvedPhysicsSpec, DiagnosticBag> {
    let mut diagnostics = DiagnosticBag::new();
    let mut modules = selection
        .preset
        .modules()
        .iter()
        .copied()
        .map(default_config)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|message| internal_resolution_error(&path, message))?;

    let mut removals = BTreeSet::new();
    for (index, model) in selection.remove.iter().copied().enumerate() {
        let item_path = path.clone().field("remove").index(index);
        if !removals.insert(model) {
            diagnostics.push(
                Diagnostic::error(
                    "case.physics.remove_duplicate",
                    format!("module '{model}' is removed more than once"),
                )
                .at(item_path),
            );
        } else if !modules.iter().any(|module| module.model == model) {
            diagnostics.push(
                Diagnostic::error(
                    "case.physics.remove_unknown",
                    format!("module '{model}' is not present in the selected preset"),
                )
                .at(item_path),
            );
        }
    }
    modules.retain(|module| !removals.contains(&module.model));

    let mut overridden = BTreeSet::new();
    for (index, patch) in selection.overrides.iter().enumerate() {
        let item_path = path.clone().field("overrides").index(index);
        if !overridden.insert(patch.model) {
            diagnostics.push(
                Diagnostic::error(
                    "case.physics.override_duplicate",
                    format!("module '{}' is overridden more than once", patch.model),
                )
                .at(item_path),
            );
            continue;
        }
        match modules
            .iter_mut()
            .find(|module| module.model == patch.model)
        {
            Some(module) => apply_patch(module, patch),
            None => diagnostics.push(
                Diagnostic::error(
                    "case.physics.override_unknown",
                    format!("module '{}' is absent after removals", patch.model),
                )
                .at(item_path.field("model")),
            ),
        }
    }

    for (index, addition) in selection.modules.iter().enumerate() {
        let item_path = path.clone().field("modules").index(index);
        if modules.iter().any(|module| module.model == addition.model) {
            diagnostics.push(
                Diagnostic::error(
                    "case.physics.module_duplicate",
                    format!("module '{}' already exists", addition.model),
                )
                .at(item_path.field("model")),
            );
            continue;
        }
        let mut module = default_config(addition.model)
            .map_err(|message| internal_resolution_error(&path, message))?;
        apply_patch(&mut module, addition);
        modules.push(module);
    }

    validate_module_parameters(&modules, &path, &mut diagnostics);
    apply_and_validate_order(&mut modules, &path, &mut diagnostics);
    validate_dependencies(&modules, &path, &mut diagnostics);
    if diagnostics.has_errors() {
        return Err(diagnostics);
    }

    let normalized_overrides = selection
        .overrides
        .iter()
        .map(normalize_config)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|message| internal_resolution_error(&path, message))?;
    let resolved_modules = modules
        .into_iter()
        .enumerate()
        .map(|(index, module)| {
            Ok(ResolvedPhysicsModule {
                model: module.model,
                order: module.order.unwrap_or(index),
                maximum_substep: module
                    .maximum_substep
                    .as_ref()
                    .map(normalize_seconds)
                    .transpose()?,
                correlation_interval_fraction: module.correlation_interval_fraction,
            })
        })
        .collect::<Result<Vec<_>, String>>()
        .map_err(|message| internal_resolution_error(&path, message))?;

    Ok(ResolvedPhysicsSpec {
        preset: selection.preset,
        remove: selection.remove.clone(),
        overrides: normalized_overrides,
        modules: resolved_modules,
    })
}

/// Validates a previously resolved physics value read from a serialized artifact.
#[must_use]
pub fn validate_resolved_physics(
    physics: &ResolvedPhysicsSpec,
    path: DiagnosticPath,
) -> DiagnosticBag {
    let mut diagnostics = DiagnosticBag::new();
    let mut modules = physics
        .modules
        .iter()
        .map(|module| PhysicsModuleConfig {
            model: module.model,
            order: Some(module.order),
            maximum_substep: module.maximum_substep.clone(),
            correlation_interval_fraction: module.correlation_interval_fraction,
        })
        .collect::<Vec<_>>();
    let mut seen = BTreeSet::new();
    for (index, module) in modules.iter().enumerate() {
        if !seen.insert(module.model) {
            diagnostics.push(
                Diagnostic::error(
                    "case.physics.module_duplicate",
                    format!("module '{}' occurs more than once", module.model),
                )
                .at(path.clone().field("modules").index(index).field("model")),
            );
        }
    }
    validate_module_parameters(&modules, &path, &mut diagnostics);
    apply_and_validate_order(&mut modules, &path, &mut diagnostics);
    validate_dependencies(&modules, &path, &mut diagnostics);
    diagnostics
}

fn default_config(model: PhysicsModuleId) -> Result<PhysicsModuleConfig, String> {
    Ok(PhysicsModuleConfig {
        model,
        order: None,
        maximum_substep: model
            .default_maximum_substep_seconds()
            .map(seconds_quantity)
            .transpose()?,
        correlation_interval_fraction: (model == PhysicsModuleId::MesoscaleMarkov).then_some(0.5),
    })
}

fn apply_patch(target: &mut PhysicsModuleConfig, patch: &PhysicsModuleConfig) {
    if patch.order.is_some() {
        target.order = patch.order;
    }
    if patch.maximum_substep.is_some() {
        target.maximum_substep.clone_from(&patch.maximum_substep);
    }
    if patch.correlation_interval_fraction.is_some() {
        target.correlation_interval_fraction = patch.correlation_interval_fraction;
    }
}

fn validate_module_parameters(
    modules: &[PhysicsModuleConfig],
    path: &DiagnosticPath,
    diagnostics: &mut DiagnosticBag,
) {
    for (index, module) in modules.iter().enumerate() {
        let module_path = path.clone().field("modules").index(index);
        match (
            module.model.maximum_substep_ceiling_seconds(),
            module.maximum_substep.as_ref(),
        ) {
            (None, Some(_)) => diagnostics.push(
                Diagnostic::error(
                    "case.physics.maximum_substep_unsupported",
                    format!("module '{}' has no maximum_substep parameter", module.model),
                )
                .at(module_path.clone().field("maximum_substep")),
            ),
            (Some(ceiling), Some(value))
                if !value.is_positive_finite() || value.value_si() > ceiling =>
            {
                diagnostics.push(
                    Diagnostic::error(
                        "case.physics.maximum_substep_invalid",
                        format!(
                            "module '{}' maximum_substep must be within (0, {ceiling}] seconds",
                            module.model
                        ),
                    )
                    .at(module_path.clone().field("maximum_substep")),
                );
            }
            (Some(_), None) => diagnostics.push(
                Diagnostic::error(
                    "case.physics.maximum_substep_missing",
                    format!("module '{}' requires maximum_substep", module.model),
                )
                .at(module_path.clone().field("maximum_substep")),
            ),
            _ => {}
        }
        match (module.model, module.correlation_interval_fraction) {
            (PhysicsModuleId::MesoscaleMarkov, Some(value))
                if value.is_finite() && (0.05..=1.0).contains(&value) => {}
            (PhysicsModuleId::MesoscaleMarkov, Some(_)) => diagnostics.push(
                Diagnostic::error(
                    "case.physics.correlation_interval_fraction_invalid",
                    "mesoscale_markov correlation_interval_fraction must be within [0.05, 1]",
                )
                .at(module_path.clone().field("correlation_interval_fraction")),
            ),
            (PhysicsModuleId::MesoscaleMarkov, None) => diagnostics.push(
                Diagnostic::error(
                    "case.physics.correlation_interval_fraction_missing",
                    "mesoscale_markov requires correlation_interval_fraction",
                )
                .at(module_path.clone().field("correlation_interval_fraction")),
            ),
            (_, Some(_)) => diagnostics.push(
                Diagnostic::error(
                    "case.physics.correlation_interval_fraction_unsupported",
                    format!(
                        "module '{}' has no correlation_interval_fraction parameter",
                        module.model
                    ),
                )
                .at(module_path.clone().field("correlation_interval_fraction")),
            ),
            (_, None) => {}
        }
    }
}

fn apply_and_validate_order(
    modules: &mut [PhysicsModuleConfig],
    path: &DiagnosticPath,
    diagnostics: &mut DiagnosticBag,
) {
    if !modules.iter().any(|module| module.order.is_some()) {
        for (index, module) in modules.iter_mut().enumerate() {
            module.order = Some(index);
        }
        return;
    }

    let mut orders = BTreeMap::new();
    for (index, module) in modules.iter().enumerate() {
        let Some(order) = module.order else {
            diagnostics.push(
                Diagnostic::error(
                    "case.physics.order_partial",
                    "when one module sets order, every final module must set order",
                )
                .at(path.clone().field("modules").index(index).field("order")),
            );
            continue;
        };
        if let Some(previous) = orders.insert(order, index) {
            diagnostics.push(
                Diagnostic::error(
                    "case.physics.order_duplicate",
                    format!("order {order} is also used by module index {previous}"),
                )
                .at(path.clone().field("modules").index(index).field("order")),
            );
        }
    }
    if orders.len() == modules.len() && orders.keys().copied().ne(0..modules.len()) {
        diagnostics.push(
            Diagnostic::error(
                "case.physics.order_noncontiguous",
                "module order must be the contiguous range 0..N-1",
            )
            .at(path.clone().field("modules")),
        );
    }
    if !diagnostics.has_errors() {
        modules.sort_by_key(|module| module.order);
    }
}

fn validate_dependencies(
    modules: &[PhysicsModuleConfig],
    path: &DiagnosticPath,
    diagnostics: &mut DiagnosticBag,
) {
    let positions = modules
        .iter()
        .enumerate()
        .map(|(index, module)| (module.model, index))
        .collect::<BTreeMap<_, _>>();
    for (index, module) in modules.iter().enumerate() {
        for dependency in module.model.requires() {
            match positions.get(dependency) {
                None => diagnostics.push(
                    Diagnostic::error(
                        "case.physics.dependency_missing",
                        format!("module '{}' requires '{dependency}'", module.model),
                    )
                    .at(path
                        .clone()
                        .field("modules")
                        .index(index)
                        .field("model")),
                ),
                Some(position) if *position >= index => diagnostics.push(
                    Diagnostic::error(
                        "case.physics.dependency_order",
                        format!("module '{dependency}' must precede '{}'", module.model),
                    )
                    .at(path
                        .clone()
                        .field("modules")
                        .index(index)
                        .field("order")),
                ),
                Some(_) => {}
            }
        }
    }
}

fn normalize_config(config: &PhysicsModuleConfig) -> Result<PhysicsModuleConfig, String> {
    Ok(PhysicsModuleConfig {
        model: config.model,
        order: config.order,
        maximum_substep: config
            .maximum_substep
            .as_ref()
            .map(normalize_seconds)
            .transpose()?,
        correlation_interval_fraction: config.correlation_interval_fraction,
    })
}

fn normalize_seconds(value: &Quantity<Time>) -> Result<Quantity<Time>, String> {
    seconds_quantity(value.value_si())
}

fn seconds_quantity(seconds: f64) -> Result<Quantity<Time>, String> {
    let unit = Unit::new("s", crate::quantity::Dimension::TIME, 1.0, 0.0)
        .map_err(|error| error.to_string())?;
    Quantity::from_si(seconds, unit).map_err(|error| error.to_string())
}

fn internal_resolution_error(path: &DiagnosticPath, message: String) -> DiagnosticBag {
    let mut diagnostics = DiagnosticBag::new();
    diagnostics.push(
        Diagnostic::error(
            "case.physics.normalization_failed",
            format!("physics quantity normalization failed: {message}"),
        )
        .at(path.clone()),
    );
    diagnostics
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use crate::quantity::{QuantityInput, UnitRegistry};

    fn duration(text: &str) -> Quantity<Time> {
        UnitRegistry::standard().parse_text(text).unwrap()
    }

    #[test]
    fn preset_merge_removes_overrides_appends_and_normalizes() {
        let selection = PhysicsSelectionSpec {
            preset: PhysicsPreset::WaterVaporTracking,
            remove: vec![
                PhysicsModuleId::SubgridOrography,
                PhysicsModuleId::MesoscaleMarkov,
                PhysicsModuleId::DeepConvectionColumn,
                PhysicsModuleId::WaterVaporExchange,
            ],
            overrides: vec![PhysicsModuleConfig {
                model: PhysicsModuleId::BoundaryLayerLangevin,
                order: None,
                maximum_substep: Some(duration("0.5 min")),
                correlation_interval_fraction: None,
            }],
            modules: Vec::new(),
        };
        let resolved =
            resolve_physics(&selection, DiagnosticPath::root().field("physics")).unwrap();
        assert_eq!(resolved.modules.len(), 1);
        assert_eq!(resolved.modules[0].order, 0);
        let step = resolved.modules[0].maximum_substep.as_ref().unwrap();
        assert_eq!(step.value_si(), 30.0);
        assert_eq!(step.source_unit().symbol(), "s");
    }

    #[test]
    fn explicit_order_is_all_or_none_and_dependency_checked() {
        let selection = PhysicsSelectionSpec {
            preset: PhysicsPreset::BuoyantRelease,
            remove: vec![
                PhysicsModuleId::SubgridOrography,
                PhysicsModuleId::BoundaryLayerLangevin,
                PhysicsModuleId::MesoscaleMarkov,
                PhysicsModuleId::DeepConvectionColumn,
            ],
            overrides: vec![PhysicsModuleConfig {
                model: PhysicsModuleId::BuoyantPlumeRise,
                order: Some(0),
                maximum_substep: None,
                correlation_interval_fraction: None,
            }],
            modules: Vec::new(),
        };
        let diagnostics =
            resolve_physics(&selection, DiagnosticPath::root().field("physics")).unwrap_err();
        assert!(
            diagnostics
                .iter()
                .any(|d| d.code() == "case.physics.order_partial")
        );
    }

    #[test]
    fn legacy_module_shape_is_rejected() {
        let yaml = r#"
preset: water_vapor_tracking
overrides:
  - model: boundary_layer_langevin
    enabled: true
    parameters: { maximum_substep: 30 s }
"#;
        assert!(serde_yml::from_str::<PhysicsSelectionSpec>(yaml).is_err());
    }

    #[test]
    fn dimensional_maximum_substep_rejects_bare_number() {
        let yaml = r#"
preset: water_vapor_tracking
overrides:
  - model: boundary_layer_langevin
    maximum_substep: 30
"#;
        assert!(serde_yml::from_str::<PhysicsSelectionSpec>(yaml).is_err());
        let _ = QuantityInput::text("30 s");
    }

    #[test]
    fn mesoscale_correlation_fraction_is_typed_and_bounded() {
        let yaml = r#"
preset: water_vapor_tracking
overrides:
  - model: mesoscale_markov
    correlation_interval_fraction: 0.25
"#;
        let selection = serde_yml::from_str::<PhysicsSelectionSpec>(yaml).unwrap();
        let resolved =
            resolve_physics(&selection, DiagnosticPath::root().field("physics")).unwrap();
        let mesoscale = resolved
            .modules
            .iter()
            .find(|module| module.model == PhysicsModuleId::MesoscaleMarkov)
            .unwrap();
        assert_eq!(mesoscale.correlation_interval_fraction, Some(0.25));

        let invalid =
            serde_yml::from_str::<PhysicsSelectionSpec>(&yaml.replace("0.25", "1.5")).unwrap();
        let diagnostics =
            resolve_physics(&invalid, DiagnosticPath::root().field("physics")).unwrap_err();
        assert!(diagnostics.iter().any(|diagnostic| {
            diagnostic.code() == "case.physics.correlation_interval_fraction_invalid"
        }));
    }
}
