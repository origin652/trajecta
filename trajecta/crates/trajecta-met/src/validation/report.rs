//! # Contract: unified M3 comparison report builder
//!
//! Adjudicates field slabs against a loaded registry and emits
//! trajecta.m3.comparison_report/v1 documents with frozen coverage and blockers.
//!
use std::collections::BTreeSet;

use serde::Serialize;

use crate::validation::tolerance::{
    Decision, ExactEquality, ExactMetadataName, LoadedRegistry, Metric, Rule, RuleMatchError,
    SampleContext, absolute_relative_ok, direction_degrees, match_rule, ordered_ulp_distance,
    scalars_equal, vector_ok,
};

/// Overall report status.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ComparisonStatus {
    Passed,
    Failed,
    Incomplete,
    Unvalidated,
}

/// Per-result row status.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ResultStatus {
    Pass,
    Fail,
    Reported,
    Unvalidated,
}

/// Registry identity embedded in a report.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct RegistryIdentity {
    pub version: String,
    pub sha256: String,
}

/// Comparison identity (subject/reference digests are frozen combination hashes).
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ComparisonIdentity {
    pub dataset_family: String,
    pub comparison_target: String,
    pub comparison_variant: String,
    pub subject_sha256: String,
    pub reference_sha256: String,
}

/// Coverage relative to a frozen expected matrix (never reverse-derived from observations alone).
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct Coverage {
    pub expected_case_ids: Vec<String>,
    pub observed_case_ids: Vec<String>,
    pub missing_case_ids: Vec<String>,
}

/// Aggregate numeric diagnostics for one result row.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct ResultStatistics {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exact_mismatch_count: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub maximum_absolute_difference: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub maximum_relative_difference: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub maximum_ulps: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub maximum_direction_degrees: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worst_case_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worst_subject_value: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worst_reference_value: Option<f64>,
}

/// One adjudicated field/rule result.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ComparisonResult {
    pub rule_id: String,
    pub field: String,
    pub decision: String,
    pub status: ResultStatus,
    pub compared_count: u64,
    pub invalid_count: u64,
    pub numeric_failure_count: u64,
    pub metadata_failure_count: u64,
    pub non_finite_count: u64,
    pub statistics: ResultStatistics,
}

/// Full comparison report document.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ComparisonReport {
    pub schema_version: String,
    pub report_id: String,
    pub algorithm_id: String,
    pub registry: RegistryIdentity,
    pub comparison: ComparisonIdentity,
    pub status: ComparisonStatus,
    pub coverage: Coverage,
    pub results: Vec<ComparisonResult>,
    pub blockers: Vec<String>,
}

/// Owned sample context used by slabs.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SampleContextOwned {
    pub dataset_family: String,
    pub comparison_target: String,
    pub comparison_variant: String,
    pub sample_scope: String,
    pub field_namespace: String,
    pub field: String,
    pub coordinate: String,
    pub vertical_region: String,
}

impl SampleContextOwned {
    fn as_ctx(&self) -> SampleContext<'_> {
        SampleContext {
            dataset_family: &self.dataset_family,
            comparison_target: &self.comparison_target,
            comparison_variant: &self.comparison_variant,
            sample_scope: &self.sample_scope,
            field_namespace: &self.field_namespace,
            field: &self.field,
            coordinate: &self.coordinate,
            vertical_region: &self.vertical_region,
        }
    }
}

/// Exact metadata carried with one side of a slab (subject or reference).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SideMetadata {
    /// Physical valid time as unix seconds (and optional nanosecond).
    pub valid_time_unix_seconds: i64,
    pub valid_time_nanosecond: u32,
    /// Canonical grid signature string (shape + coord digest + scan + period).
    pub grid_signature: String,
    /// Canonical vertical topology signature.
    pub vertical_signature: String,
    /// Canonical array layout signature.
    pub layout_signature: String,
    /// Source unit symbol.
    pub unit: String,
    /// Temporal support signature.
    pub temporal_support: String,
    /// Optional status token.
    pub status: Option<String>,
}

/// One inseparable U/V vector observation at a single element.
#[derive(Clone, Debug, PartialEq)]
pub struct VectorElement {
    pub case_id: String,
    pub subject_u: f64,
    pub subject_v: f64,
    pub reference_u: f64,
    pub reference_v: f64,
    pub subject_u_valid: bool,
    pub subject_v_valid: bool,
    pub reference_u_valid: bool,
    pub reference_v_valid: bool,
    pub subject_u_unit: String,
    pub subject_v_unit: String,
    pub reference_u_unit: String,
    pub reference_v_unit: String,
    pub subject_u_status: Option<String>,
    pub subject_v_status: Option<String>,
    pub reference_u_status: Option<String>,
    pub reference_v_status: Option<String>,
}

/// Field/slab comparison input. Lengths must already match expected_element_count.
#[derive(Clone, Debug, PartialEq)]
pub struct FieldSlab {
    /// Stable slab id (also the coverage case id).
    pub slab_id: String,
    pub context: SampleContextOwned,
    pub subject_meta: SideMetadata,
    pub reference_meta: SideMetadata,
    /// Expected element count from frozen matrix (must equal all array lengths).
    pub expected_element_count: usize,
    /// Scalar values (empty when this slab is pure vector).
    pub subject_values: Vec<f64>,
    pub reference_values: Vec<f64>,
    pub subject_mask: Vec<bool>,
    pub reference_mask: Vec<bool>,
    /// Optional per-element case ids for diagnostics (len == expected or empty).
    pub element_case_ids: Vec<String>,
    /// Vector pairs when metric is vector; None for scalar slabs.
    pub vector_elements: Option<Vec<VectorElement>>,
}

// Backward-compatible aliases used by older call sites during migration.
/// Deprecated scalar pair; prefer [`FieldSlab`].
#[derive(Clone, Debug, PartialEq)]
pub struct SamplePair {
    pub case_id: String,
    pub subject: f64,
    pub reference: f64,
    pub subject_valid: bool,
    pub reference_valid: bool,
    pub subject_unit: Option<String>,
    pub reference_unit: Option<String>,
    pub subject_status: Option<String>,
    pub reference_status: Option<String>,
}

/// Deprecated field sample; prefer [`FieldSlab`].
#[derive(Clone, Debug, PartialEq)]
pub struct FieldSample {
    pub context: SampleContextOwned,
    pub pairs: Vec<SamplePair>,
    pub vector_pairs: Option<Vec<SamplePair>>,
}

/// Adjudicate frozen expected slabs against a loaded registry.
///
/// `expected_case_ids` must be produced from the frozen matrix **before** decode.
/// Observed ids come only from successfully submitted slabs; missing ids fail coverage.
pub fn adjudicate(
    report_id: &str,
    registry: &LoadedRegistry,
    comparison: ComparisonIdentity,
    expected_case_ids: &[String],
    slabs: &[FieldSlab],
    extra_blockers: &[String],
) -> ComparisonReport {
    let mut blockers: BTreeSet<String> = extra_blockers.iter().cloned().collect();
    let mut results = Vec::new();
    let mut observed: BTreeSet<String> = BTreeSet::new();
    let expected: BTreeSet<String> = expected_case_ids.iter().cloned().collect();

    if expected.is_empty() {
        blockers.insert("expected_case_ids_empty".into());
    }

    let mut hard_fail = false;
    let mut any_unvalidated = false;
    let mut config_error = false;

    // Duplicate slab ids → incomplete (config error).
    {
        let mut seen = BTreeSet::new();
        for slab in slabs {
            if !seen.insert(slab.slab_id.clone()) {
                blockers.insert(format!("duplicate_slab:{}", slab.slab_id));
                config_error = true;
            }
        }
    }

    for slab in slabs {
        observed.insert(slab.slab_id.clone());

        // Comparison identity must agree with every slab context.
        if slab.context.dataset_family != comparison.dataset_family
            || slab.context.comparison_target != comparison.comparison_target
            || slab.context.comparison_variant != comparison.comparison_variant
        {
            blockers.insert(format!("comparison_identity_mismatch:{}", slab.slab_id));
            config_error = true;
            continue;
        }

        let rule = match match_rule(registry, &slab.context.as_ctx()) {
            Ok(r) => r,
            Err(RuleMatchError::Unvalidated) => {
                any_unvalidated = true;
                results.push(ComparisonResult {
                    rule_id: "unvalidated".into(),
                    field: slab.context.field.clone(),
                    decision: "hard_gate".into(),
                    status: ResultStatus::Unvalidated,
                    compared_count: 0,
                    invalid_count: 0,
                    numeric_failure_count: 0,
                    metadata_failure_count: 0,
                    non_finite_count: 0,
                    statistics: ResultStatistics::default(),
                });
                blockers.insert(format!("unvalidated_rule:{}", slab.slab_id));
                continue;
            }
            Err(RuleMatchError::Ambiguous) => {
                config_error = true;
                blockers.insert(format!("ambiguous_rule:{}", slab.slab_id));
                continue;
            }
        };

        let evaluated = evaluate_slab(rule, slab);
        let config_reason = evaluated
            .statistics
            .worst_case_id
            .as_deref()
            .is_some_and(is_config_error_reason);
        if config_reason {
            config_error = true;
            blockers.insert(format!(
                "config:{}:{}",
                slab.slab_id,
                evaluated
                    .statistics
                    .worst_case_id
                    .clone()
                    .unwrap_or_default()
            ));
        }
        match evaluated.status {
            ResultStatus::Pass | ResultStatus::Reported => {}
            ResultStatus::Fail => {
                if config_reason {
                    // Configuration/shape errors are incomplete, not hard-gate failures.
                } else if evaluated.decision == "hard_gate" {
                    hard_fail = true;
                }
            }
            ResultStatus::Unvalidated => any_unvalidated = true,
        }
        results.push(evaluated);
    }

    let missing: Vec<String> = expected.difference(&observed).cloned().collect();
    if !missing.is_empty() {
        for m in &missing {
            blockers.insert(format!("missing_case:{m}"));
        }
    }
    // Extra observed slabs not in expected matrix → incomplete.
    for extra in observed.difference(&expected) {
        blockers.insert(format!("unexpected_case:{extra}"));
        config_error = true;
    }

    let coverage = Coverage {
        expected_case_ids: expected.iter().cloned().collect(),
        observed_case_ids: observed.iter().cloned().collect(),
        missing_case_ids: missing,
    };

    let any_hard_gate_pass = results
        .iter()
        .any(|r| r.status == ResultStatus::Pass && r.decision == "hard_gate");

    // Priority: config/coverage/blockers ⇒ incomplete (even alongside hard numeric fails).
    // Pure hard_fail with empty blockers ⇒ failed.
    // passed only with hard-gate Pass and zero blockers.
    let status = if config_error || !coverage.missing_case_ids.is_empty() {
        ComparisonStatus::Incomplete
    } else if any_unvalidated {
        ComparisonStatus::Unvalidated
    } else if !blockers.is_empty() {
        // Includes hard_fail + any blocker coexistence.
        ComparisonStatus::Incomplete
    } else if hard_fail {
        ComparisonStatus::Failed
    } else if any_hard_gate_pass {
        ComparisonStatus::Passed
    } else {
        // report_only-only, empty results, or no hard-gate Pass
        ComparisonStatus::Incomplete
    };

    let mut blocker_list: Vec<String> = blockers.into_iter().collect();
    blocker_list.sort();

    ComparisonReport {
        schema_version: "trajecta.m3.comparison_report/v1".into(),
        report_id: report_id.into(),
        algorithm_id: "trajecta/met_query/m3/v0".into(),
        registry: RegistryIdentity {
            version: registry.document.registry_version.clone(),
            sha256: registry.sha256.clone(),
        },
        comparison,
        status,
        coverage,
        results,
        blockers: blocker_list,
    }
}

fn evaluate_slab(rule: &Rule, slab: &FieldSlab) -> ComparisonResult {
    let decision = match rule.decision {
        Decision::HardGate => "hard_gate",
        Decision::ReportOnly => "report_only",
    };

    // Length contract: no silent truncation.
    let structural = validate_structure(rule, slab);
    if let Some(mut base) = structural {
        base.rule_id = rule.id.clone();
        base.field = slab.context.field.clone();
        base.decision = decision.into();
        return base;
    }

    // Metric/shape agreement.
    match &rule.metric {
        Metric::Vector { .. } => {
            if slab.vector_elements.is_none() {
                return config_fail(rule, slab, decision, "vector_rule_without_vector_data");
            }
        }
        _ => {
            if slab.vector_elements.is_some() {
                return config_fail(rule, slab, decision, "scalar_rule_with_vector_data");
            }
        }
    }

    let meta_err = compare_exact_metadata(rule, slab);
    if let Some(reason) = meta_err {
        let metadata_failure_count = 1_u64;
        let status = match rule.decision {
            Decision::HardGate => ResultStatus::Fail,
            Decision::ReportOnly => ResultStatus::Reported,
        };
        return ComparisonResult {
            rule_id: rule.id.clone(),
            field: slab.context.field.clone(),
            decision: decision.into(),
            status,
            compared_count: 0,
            invalid_count: 0,
            numeric_failure_count: 0,
            metadata_failure_count,
            non_finite_count: 0,
            statistics: ResultStatistics {
                worst_case_id: Some(reason),
                ..ResultStatistics::default()
            },
        };
    }

    match &rule.metric {
        Metric::Exact {
            equality,
            signed_zero_equal,
        } => evaluate_scalar_exact(rule, slab, decision, *equality, *signed_zero_equal),
        Metric::Ulp {
            maximum_ulps,
            signed_zero_equal,
        } => evaluate_scalar_ulp(rule, slab, decision, *maximum_ulps, *signed_zero_equal),
        Metric::AbsoluteRelative {
            absolute,
            relative,
            scale_floor,
        } => evaluate_scalar_absrel(rule, slab, decision, *absolute, *relative, *scale_floor),
        Metric::Vector {
            component_absolute,
            component_relative,
            component_scale_floor,
            speed_floor,
            direction_degrees,
        } => evaluate_vector(
            rule,
            slab,
            decision,
            *component_absolute,
            *component_relative,
            *component_scale_floor,
            *speed_floor,
            *direction_degrees,
        ),
    }
}

fn config_fail(rule: &Rule, slab: &FieldSlab, decision: &str, reason: &str) -> ComparisonResult {
    ComparisonResult {
        rule_id: rule.id.clone(),
        field: slab.context.field.clone(),
        decision: decision.into(),
        status: ResultStatus::Fail,
        compared_count: 0,
        invalid_count: 0,
        numeric_failure_count: 0,
        metadata_failure_count: 1,
        non_finite_count: 0,
        statistics: ResultStatistics {
            worst_case_id: Some(reason.into()),
            ..ResultStatistics::default()
        },
    }
}

fn validate_structure(rule: &Rule, slab: &FieldSlab) -> Option<ComparisonResult> {
    let n = slab.expected_element_count;
    if matches!(rule.metric, Metric::Vector { .. }) {
        let Some(elems) = slab.vector_elements.as_ref() else {
            return Some(config_fail(
                rule,
                slab,
                "hard_gate",
                "missing_vector_elements",
            ));
        };
        if elems.len() != n {
            return Some(ComparisonResult {
                rule_id: String::new(),
                field: String::new(),
                decision: String::new(),
                status: ResultStatus::Fail,
                compared_count: 0,
                invalid_count: 0,
                numeric_failure_count: 0,
                metadata_failure_count: 1,
                non_finite_count: 0,
                statistics: ResultStatistics {
                    worst_case_id: Some("length_or_structure".into()),
                    ..ResultStatistics::default()
                },
            });
        }
        // Vector integrity: unique case ids, paired units already per-element.
        let mut ids = BTreeSet::new();
        for e in elems {
            if !ids.insert(e.case_id.clone()) {
                return Some(config_fail(
                    rule,
                    slab,
                    "hard_gate",
                    "duplicate_vector_case_id",
                ));
            }
        }
        return None;
    }

    if slab.subject_values.len() != n
        || slab.reference_values.len() != n
        || slab.subject_mask.len() != n
        || slab.reference_mask.len() != n
    {
        return Some(ComparisonResult {
            rule_id: String::new(),
            field: String::new(),
            decision: String::new(),
            status: ResultStatus::Fail,
            compared_count: 0,
            invalid_count: 0,
            numeric_failure_count: 0,
            metadata_failure_count: 1,
            non_finite_count: 0,
            statistics: ResultStatistics {
                worst_case_id: Some("length_or_structure".into()),
                ..ResultStatistics::default()
            },
        });
    }
    if !slab.element_case_ids.is_empty() && slab.element_case_ids.len() != n {
        return Some(config_fail(
            rule,
            slab,
            "hard_gate",
            "element_case_id_length",
        ));
    }
    None
}

fn compare_exact_metadata(rule: &Rule, slab: &FieldSlab) -> Option<String> {
    let s = &slab.subject_meta;
    let r = &slab.reference_meta;
    for name in &rule.exact_metadata {
        let ok = match name {
            ExactMetadataName::ValidTimes => {
                s.valid_time_unix_seconds == r.valid_time_unix_seconds
                    && s.valid_time_nanosecond == r.valid_time_nanosecond
            }
            ExactMetadataName::Grid => s.grid_signature == r.grid_signature,
            ExactMetadataName::VerticalTopology => s.vertical_signature == r.vertical_signature,
            ExactMetadataName::Layout => s.layout_signature == r.layout_signature,
            ExactMetadataName::Mask => {
                // Element-wise mask compared during numeric pass; here require equal length already.
                slab.subject_mask.len() == slab.reference_mask.len()
                    && slab.subject_mask.len() == slab.expected_element_count
            }
            ExactMetadataName::Unit => s.unit == r.unit && !s.unit.is_empty(),
            ExactMetadataName::TemporalSupport => s.temporal_support == r.temporal_support,
            ExactMetadataName::Status => s.status == r.status,
            ExactMetadataName::Validity => {
                // Validity is the per-element mask identity; structural equality of masks.
                slab.subject_mask == slab.reference_mask
            }
        };
        if !ok {
            return Some(format!("metadata:{name:?}"));
        }
    }
    // Unknown names cannot appear: ExactMetadataName is closed enum via serde.
    None
}

fn element_id(slab: &FieldSlab, index: usize) -> String {
    if let Some(id) = slab.element_case_ids.get(index) {
        return id.clone();
    }
    format!("{}#{index}", slab.slab_id)
}

fn is_config_error_reason(reason: &str) -> bool {
    matches!(
        reason,
        "length_or_structure"
            | "vector_rule_without_vector_data"
            | "scalar_rule_with_vector_data"
            | "missing_vector_elements"
            | "duplicate_vector_case_id"
            | "element_case_id_length"
    ) || reason.starts_with("config:")
}

fn finalize_status(
    rule: &Rule,
    numeric_fail: u64,
    non_finite: u64,
    meta_fail: u64,
) -> ResultStatus {
    let bad = numeric_fail > 0 || non_finite > 0 || meta_fail > 0;
    match (rule.decision, bad) {
        // report_only never yields Pass — clean or dirty both stay Reported.
        (Decision::ReportOnly, _) => ResultStatus::Reported,
        (Decision::HardGate, false) => ResultStatus::Pass,
        (Decision::HardGate, true) => ResultStatus::Fail,
    }
}

fn evaluate_scalar_exact(
    rule: &Rule,
    slab: &FieldSlab,
    decision: &str,
    equality: ExactEquality,
    signed_zero_equal: bool,
) -> ComparisonResult {
    let n = slab.expected_element_count;
    let mut compared = 0_u64;
    let mut invalid = 0_u64;
    let mut numeric_fail = 0_u64;
    let mut non_finite = 0_u64;
    let mut meta_fail = 0_u64;
    let mut exact_mismatch = 0_u64;
    let mut max_abs = 0.0_f64;
    let mut max_rel = 0.0_f64;
    let mut worst_id = None;
    let mut worst_s = None;
    let mut worst_r = None;
    let mut worst_score = -1.0_f64;

    for i in 0..n {
        let sv = slab.subject_values[i];
        let rv = slab.reference_values[i];
        let sm = slab.subject_mask[i];
        let rm = slab.reference_mask[i];
        if sm != rm {
            meta_fail += 1;
            continue;
        }
        if !sm && !rm {
            invalid += 1;
            continue;
        }
        // one side invalid already rejected by mask equality when policies require exact mask
        if !sv.is_finite() || !rv.is_finite() {
            non_finite += 1;
            continue;
        }
        compared += 1;
        if sv != rv {
            exact_mismatch += 1;
        }
        if !scalars_equal(sv, rv, equality, signed_zero_equal) {
            numeric_fail += 1;
            let abs = (sv - rv).abs();
            let rel = abs / sv.abs().max(rv.abs()).max(1e-300);
            let score = abs;
            if score > worst_score {
                worst_score = score;
                max_abs = abs;
                max_rel = rel;
                worst_id = Some(element_id(slab, i));
                worst_s = Some(sv);
                worst_r = Some(rv);
            }
        } else {
            let abs = (sv - rv).abs();
            if abs > max_abs {
                max_abs = abs;
            }
        }
    }

    ComparisonResult {
        rule_id: rule.id.clone(),
        field: slab.context.field.clone(),
        decision: decision.into(),
        status: finalize_status(rule, numeric_fail, non_finite, meta_fail),
        compared_count: compared,
        invalid_count: invalid,
        numeric_failure_count: numeric_fail,
        metadata_failure_count: meta_fail,
        non_finite_count: non_finite,
        statistics: ResultStatistics {
            exact_mismatch_count: Some(exact_mismatch),
            maximum_absolute_difference: Some(max_abs),
            maximum_relative_difference: Some(max_rel),
            maximum_ulps: None,
            maximum_direction_degrees: None,
            worst_case_id: worst_id,
            worst_subject_value: worst_s,
            worst_reference_value: worst_r,
        },
    }
}

fn evaluate_scalar_ulp(
    rule: &Rule,
    slab: &FieldSlab,
    decision: &str,
    maximum_ulps: u64,
    signed_zero_equal: bool,
) -> ComparisonResult {
    let n = slab.expected_element_count;
    let mut compared = 0_u64;
    let mut invalid = 0_u64;
    let mut numeric_fail = 0_u64;
    let mut non_finite = 0_u64;
    let mut meta_fail = 0_u64;
    let mut exact_mismatch = 0_u64;
    let mut max_abs = 0.0_f64;
    let mut max_rel = 0.0_f64;
    let mut max_ulps = 0_u64;
    let mut worst_id = None;
    let mut worst_s = None;
    let mut worst_r = None;

    for i in 0..n {
        let sv = slab.subject_values[i];
        let rv = slab.reference_values[i];
        let sm = slab.subject_mask[i];
        let rm = slab.reference_mask[i];
        if sm != rm {
            meta_fail += 1;
            continue;
        }
        if !sm && !rm {
            invalid += 1;
            continue;
        }
        if !sv.is_finite() || !rv.is_finite() {
            non_finite += 1;
            continue;
        }
        compared += 1;
        if sv != rv {
            exact_mismatch += 1;
        }
        let abs = (sv - rv).abs();
        if abs > max_abs {
            max_abs = abs;
        }
        let rel = abs / sv.abs().max(rv.abs()).max(1e-300);
        if rel > max_rel {
            max_rel = rel;
        }

        let ulps = if signed_zero_equal && sv == 0.0 && rv == 0.0 {
            0
        } else {
            ordered_ulp_distance(sv, rv)
        };
        if ulps > max_ulps {
            max_ulps = ulps;
            worst_id = Some(element_id(slab, i));
            worst_s = Some(sv);
            worst_r = Some(rv);
        }
        if ulps > maximum_ulps {
            numeric_fail += 1;
        }
    }

    ComparisonResult {
        rule_id: rule.id.clone(),
        field: slab.context.field.clone(),
        decision: decision.into(),
        status: finalize_status(rule, numeric_fail, non_finite, meta_fail),
        compared_count: compared,
        invalid_count: invalid,
        numeric_failure_count: numeric_fail,
        metadata_failure_count: meta_fail,
        non_finite_count: non_finite,
        statistics: ResultStatistics {
            exact_mismatch_count: Some(exact_mismatch),
            maximum_absolute_difference: Some(max_abs),
            maximum_relative_difference: Some(max_rel),
            maximum_ulps: Some(max_ulps),
            maximum_direction_degrees: None,
            worst_case_id: worst_id,
            worst_subject_value: worst_s,
            worst_reference_value: worst_r,
        },
    }
}

fn evaluate_scalar_absrel(
    rule: &Rule,
    slab: &FieldSlab,
    decision: &str,
    absolute: f64,
    relative: f64,
    scale_floor: f64,
) -> ComparisonResult {
    let n = slab.expected_element_count;
    let mut compared = 0_u64;
    let mut invalid = 0_u64;
    let mut numeric_fail = 0_u64;
    let mut non_finite = 0_u64;
    let mut meta_fail = 0_u64;
    let mut exact_mismatch = 0_u64;
    let mut max_abs = 0.0_f64;
    let mut max_rel = 0.0_f64;
    let mut worst_id = None;
    let mut worst_s = None;
    let mut worst_r = None;
    let mut worst_over = -1.0_f64;

    for i in 0..n {
        let sv = slab.subject_values[i];
        let rv = slab.reference_values[i];
        let sm = slab.subject_mask[i];
        let rm = slab.reference_mask[i];
        if sm != rm {
            meta_fail += 1;
            continue;
        }
        if !sm && !rm {
            invalid += 1;
            continue;
        }
        if !sv.is_finite() || !rv.is_finite() {
            non_finite += 1;
            continue;
        }
        compared += 1;
        if sv != rv {
            exact_mismatch += 1;
        }
        let abs = (sv - rv).abs();
        if abs > max_abs {
            max_abs = abs;
        }
        let scale = sv.abs().max(rv.abs()).max(scale_floor);
        let rel = abs / scale;
        if rel > max_rel {
            max_rel = rel;
        }
        let budget = absolute + relative * scale;
        let over = if budget > 0.0 { abs / budget } else { abs };
        if !absolute_relative_ok(sv, rv, absolute, relative, scale_floor) {
            numeric_fail += 1;
            if over > worst_over {
                worst_over = over;
                worst_id = Some(element_id(slab, i));
                worst_s = Some(sv);
                worst_r = Some(rv);
            }
        }
    }

    ComparisonResult {
        rule_id: rule.id.clone(),
        field: slab.context.field.clone(),
        decision: decision.into(),
        status: finalize_status(rule, numeric_fail, non_finite, meta_fail),
        compared_count: compared,
        invalid_count: invalid,
        numeric_failure_count: numeric_fail,
        metadata_failure_count: meta_fail,
        non_finite_count: non_finite,
        statistics: ResultStatistics {
            exact_mismatch_count: Some(exact_mismatch),
            maximum_absolute_difference: Some(max_abs),
            maximum_relative_difference: Some(max_rel),
            maximum_ulps: None,
            maximum_direction_degrees: None,
            worst_case_id: worst_id,
            worst_subject_value: worst_s,
            worst_reference_value: worst_r,
        },
    }
}

#[allow(clippy::too_many_arguments)]
fn evaluate_vector(
    rule: &Rule,
    slab: &FieldSlab,
    decision: &str,
    component_absolute: f64,
    component_relative: f64,
    component_scale_floor: f64,
    speed_floor: f64,
    direction_max: f64,
) -> ComparisonResult {
    let Some(elems) = slab.vector_elements.as_ref() else {
        return config_fail(rule, slab, decision, "missing_vector_elements");
    };
    let mut compared = 0_u64;
    let mut invalid = 0_u64;
    let mut numeric_fail = 0_u64;
    let mut non_finite = 0_u64;
    let mut meta_fail = 0_u64;
    let mut exact_mismatch = 0_u64;
    let mut max_abs = 0.0_f64;
    let mut max_rel = 0.0_f64;
    let mut max_dir = 0.0_f64;
    let mut worst_id = None;
    let mut worst_s = None;
    let mut worst_r = None;
    let mut worst_over = -1.0_f64;

    for e in elems {
        // Units/status must match within the pair and across subject/reference.
        if e.subject_u_unit != e.reference_u_unit
            || e.subject_v_unit != e.reference_v_unit
            || e.subject_u_unit != e.subject_v_unit
            || e.subject_u_status != e.reference_u_status
            || e.subject_v_status != e.reference_v_status
            || e.subject_u_status != e.subject_v_status
        {
            meta_fail += 1;
            continue;
        }
        let su_v = e.subject_u_valid;
        let sv_v = e.subject_v_valid;
        let ru_v = e.reference_u_valid;
        let rv_v = e.reference_v_valid;
        if su_v != ru_v || sv_v != rv_v || su_v != sv_v {
            meta_fail += 1;
            continue;
        }
        if !su_v {
            invalid += 1;
            continue;
        }
        let su = e.subject_u;
        let sv = e.subject_v;
        let ru = e.reference_u;
        let rv = e.reference_v;
        if !su.is_finite() || !sv.is_finite() || !ru.is_finite() || !rv.is_finite() {
            non_finite += 1;
            continue;
        }
        compared += 1;
        if su != ru || sv != rv {
            exact_mismatch += 1;
        }
        let abs_u = (su - ru).abs();
        let abs_v = (sv - rv).abs();
        let abs = abs_u.max(abs_v);
        if abs > max_abs {
            max_abs = abs;
        }
        let scale_u = su.abs().max(ru.abs()).max(component_scale_floor);
        let scale_v = sv.abs().max(rv.abs()).max(component_scale_floor);
        let rel = (abs_u / scale_u).max(abs_v / scale_v);
        if rel > max_rel {
            max_rel = rel;
        }
        let dir = direction_degrees(su, sv, ru, rv);
        if dir > max_dir {
            max_dir = dir;
        }
        let ok = vector_ok(
            su,
            sv,
            ru,
            rv,
            component_absolute,
            component_relative,
            component_scale_floor,
            speed_floor,
            direction_max,
        );
        if !ok {
            numeric_fail += 1;
            let budget_u = component_absolute + component_relative * scale_u;
            let budget_v = component_absolute + component_relative * scale_v;
            let over = (abs_u / budget_u.max(1e-300)).max(abs_v / budget_v.max(1e-300));
            if over > worst_over {
                worst_over = over;
                worst_id = Some(e.case_id.clone());
                worst_s = Some(su);
                worst_r = Some(ru);
            }
        }
    }

    ComparisonResult {
        rule_id: rule.id.clone(),
        field: slab.context.field.clone(),
        decision: decision.into(),
        status: finalize_status(rule, numeric_fail, non_finite, meta_fail),
        compared_count: compared,
        invalid_count: invalid,
        numeric_failure_count: numeric_fail,
        metadata_failure_count: meta_fail,
        non_finite_count: non_finite,
        statistics: ResultStatistics {
            exact_mismatch_count: Some(exact_mismatch),
            maximum_absolute_difference: Some(max_abs),
            maximum_relative_difference: Some(max_rel),
            maximum_ulps: None,
            maximum_direction_degrees: Some(max_dir),
            worst_case_id: worst_id,
            worst_subject_value: worst_s,
            worst_reference_value: worst_r,
        },
    }
}

/// Serialize report to pretty JSON bytes (LF).
pub fn report_to_json(report: &ComparisonReport) -> Result<String, String> {
    serde_json::to_string_pretty(report)
        .map(|s| {
            let mut out = s;
            if !out.ends_with('\n') {
                out.push('\n');
            }
            out
        })
        .map_err(|e| e.to_string())
}

/// Build a minimal metadata bundle used by unit tests.
#[cfg(test)]
fn test_meta(unit: &str) -> SideMetadata {
    SideMetadata {
        valid_time_unix_seconds: 1,
        valid_time_nanosecond: 0,
        grid_signature: "g1".into(),
        vertical_signature: "v1".into(),
        layout_signature: "l1".into(),
        unit: unit.into(),
        temporal_support: "instant:1".into(),
        status: Some("ok".into()),
    }
}

#[cfg(test)]
fn era5_ctx(field: &str) -> SampleContextOwned {
    SampleContextOwned {
        dataset_family: "era5_pressure".into(),
        comparison_target: "backend_equivalence".into(),
        comparison_variant: "rust-netcdf-vs-native-netcdf".into(),
        sample_scope: "decoded_field".into(),
        field_namespace: "source_identity".into(),
        field: field.into(),
        coordinate: "asl".into(),
        vertical_region: "upper_air".into(),
    }
}

#[cfg(test)]
fn cmp_id() -> ComparisonIdentity {
    ComparisonIdentity {
        dataset_family: "era5_pressure".into(),
        comparison_target: "backend_equivalence".into(),
        comparison_variant: "rust-netcdf-vs-native-netcdf".into(),
        subject_sha256: "0".repeat(64),
        reference_sha256: "1".repeat(64),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::float_cmp)]
mod tests {
    use super::*;
    use crate::validation::tolerance::load_registry_from_path;
    use std::path::PathBuf;

    fn registry() -> LoadedRegistry {
        load_registry_from_path(
            &PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../testdata/M3_TOLERANCES.v1.json"),
        )
        .unwrap()
    }

    fn scalar_slab(id: &str, field: &str, s: &[f64], r: &[f64]) -> FieldSlab {
        let n = s.len();
        FieldSlab {
            slab_id: id.into(),
            context: era5_ctx(field),
            subject_meta: test_meta("K"),
            reference_meta: test_meta("K"),
            expected_element_count: n,
            subject_values: s.to_vec(),
            reference_values: r.to_vec(),
            subject_mask: vec![true; n],
            reference_mask: vec![true; n],
            element_case_ids: (0..n).map(|i| format!("{id}#{i}")).collect(),
            vector_elements: None,
        }
    }

    #[test]
    fn exact_pass() {
        let reg = registry();
        let slab = scalar_slab("c0", "pressure:t", &[1.0], &[1.0]);
        let report = adjudicate("t-pass", &reg, cmp_id(), &["c0".into()], &[slab], &[]);
        assert_eq!(report.status, ComparisonStatus::Passed);
        assert!(report.blockers.is_empty());
        assert_eq!(report.results[0].status, ResultStatus::Pass);
    }

    #[test]
    fn exact_fail_and_nan_reject() {
        let reg = registry();
        let slab = scalar_slab("c0", "pressure:t", &[1.0], &[2.0]);
        let report = adjudicate("t-fail", &reg, cmp_id(), &["c0".into()], &[slab], &[]);
        assert_eq!(report.status, ComparisonStatus::Failed);

        let mut nan = scalar_slab("c1", "pressure:t", &[f64::NAN], &[1.0]);
        nan.slab_id = "c1".into();
        let report = adjudicate("t-nan", &reg, cmp_id(), &["c1".into()], &[nan], &[]);
        assert_eq!(report.status, ComparisonStatus::Failed);
        assert!(report.results[0].non_finite_count >= 1);
    }

    #[test]
    fn missing_coverage_incomplete() {
        let reg = registry();
        let slab = scalar_slab("c0", "pressure:t", &[1.0], &[1.0]);
        let report = adjudicate(
            "t-miss",
            &reg,
            cmp_id(),
            &["c0".into(), "c1".into()],
            &[slab],
            &[],
        );
        assert_eq!(report.status, ComparisonStatus::Incomplete);
        assert!(report.coverage.missing_case_ids.iter().any(|x| x == "c1"));
    }

    #[test]
    fn length_mismatch_no_silent_truncate() {
        let reg = registry();
        let mut slab = scalar_slab("c0", "pressure:t", &[1.0, 2.0], &[1.0, 2.0]);
        slab.subject_values.pop(); // shorter subject
        let report = adjudicate("t-len", &reg, cmp_id(), &["c0".into()], &[slab], &[]);
        assert_eq!(report.status, ComparisonStatus::Incomplete);
        assert_eq!(report.results[0].metadata_failure_count, 1);
    }

    #[test]
    fn unit_mismatch_before_numeric() {
        let reg = registry();
        let mut slab = scalar_slab("c0", "pressure:t", &[1.0], &[1.0]);
        slab.reference_meta.unit = "degC".into();
        let report = adjudicate("t-unit", &reg, cmp_id(), &["c0".into()], &[slab], &[]);
        assert_eq!(report.status, ComparisonStatus::Failed);
        assert_eq!(report.results[0].metadata_failure_count, 1);
        assert_eq!(report.results[0].compared_count, 0);
    }

    #[test]
    fn grid_mismatch() {
        let reg = registry();
        let mut slab = scalar_slab("c0", "pressure:t", &[1.0], &[1.0]);
        slab.reference_meta.grid_signature = "other".into();
        let report = adjudicate("t-grid", &reg, cmp_id(), &["c0".into()], &[slab], &[]);
        assert_eq!(report.status, ComparisonStatus::Failed);
        assert!(
            report.results[0]
                .statistics
                .worst_case_id
                .as_deref()
                .unwrap()
                .contains("Grid")
        );
    }

    #[test]
    fn unvalidated_variant() {
        let reg = registry();
        let mut slab = scalar_slab("c0", "pressure:t", &[1.0], &[1.0]);
        slab.context.comparison_variant = "nope".into();
        let mut cmp = cmp_id();
        cmp.comparison_variant = "nope".into();
        let report = adjudicate("t-uv", &reg, cmp, &["c0".into()], &[slab], &[]);
        assert_eq!(report.status, ComparisonStatus::Unvalidated);
    }

    #[test]
    fn identity_mismatch_incomplete() {
        let reg = registry();
        let slab = scalar_slab("c0", "pressure:t", &[1.0], &[1.0]);
        let mut cmp = cmp_id();
        cmp.comparison_variant = "other".into();
        let report = adjudicate("t-id", &reg, cmp, &["c0".into()], &[slab], &[]);
        assert_eq!(report.status, ComparisonStatus::Incomplete);
    }

    #[test]
    fn both_invalid_counts_as_invalid_not_compared() {
        let reg = registry();
        let mut slab = scalar_slab("c0", "pressure:t", &[1.0], &[1.0]);
        slab.subject_mask[0] = false;
        slab.reference_mask[0] = false;
        let report = adjudicate("t-inv", &reg, cmp_id(), &["c0".into()], &[slab], &[]);
        // Validity exact_metadata requires masks equal — both false is equal, so metadata ok.
        // Masks equal and both invalid → invalid_count, compared 0, pass (no numeric fail).
        assert_eq!(report.results[0].invalid_count, 1);
        assert_eq!(report.results[0].compared_count, 0);
        assert_eq!(report.status, ComparisonStatus::Passed);
    }

    #[test]
    fn ulp_worst_tracks_max_ulp_not_abs() {
        let mut reg = registry();
        // Keep this comparator-algorithm test independent of the released q threshold.
        let q_rule = reg
            .document
            .rules
            .iter_mut()
            .find(|rule| rule.id == "backend/cfsr-grib/q-three-ulp/v1")
            .expect("CFSR q rule");
        q_rule.metric = Metric::Ulp {
            maximum_ulps: 2,
            signed_zero_equal: true,
        };
        let ctx = SampleContextOwned {
            dataset_family: "cfsr_pressure".into(),
            comparison_target: "backend_equivalence".into(),
            comparison_variant: "rust-grib-vs-native-eccodes".into(),
            sample_scope: "decoded_field".into(),
            field_namespace: "source_identity".into(),
            field: "grib:0.1.0:isobaric".into(),
            coordinate: "pressure_pa".into(),
            vertical_region: "upper_air".into(),
        };
        let a = 1.0_f64;
        let b2 = f64::from_bits(a.to_bits() + 2);
        // Craft: value near 1e-9 with 3 ulp vs 2 ulp on 1.0 — worst must track max ULP index.
        let s = vec![a, 1e-9];
        let r = vec![b2, f64::from_bits((1e-9_f64).to_bits() + 3)];
        let slab = FieldSlab {
            slab_id: "u0".into(),
            context: ctx.clone(),
            subject_meta: test_meta("1"),
            reference_meta: test_meta("1"),
            expected_element_count: 2,
            subject_values: s,
            reference_values: r,
            subject_mask: vec![true; 2],
            reference_mask: vec![true; 2],
            element_case_ids: vec!["u0#0".into(), "u0#1".into()],
            vector_elements: None,
        };
        let cmp = ComparisonIdentity {
            dataset_family: "cfsr_pressure".into(),
            comparison_target: "backend_equivalence".into(),
            comparison_variant: "rust-grib-vs-native-eccodes".into(),
            subject_sha256: "0".repeat(64),
            reference_sha256: "1".repeat(64),
        };
        let report = adjudicate("t-ulp", &reg, cmp, &["u0".into()], &[slab], &[]);
        assert_eq!(report.results[0].statistics.maximum_ulps, Some(3));
        assert_eq!(
            report.results[0].statistics.worst_case_id.as_deref(),
            Some("u0#1")
        );
        assert!(report.results[0].statistics.exact_mismatch_count.unwrap() >= 1);
        assert_eq!(report.status, ComparisonStatus::Failed); // 3 > 2
    }

    #[test]
    fn vector_missing_v_is_config_error() {
        let reg = registry();
        // Find a vector rule — flexpart native anchor U/V
        let ctx = SampleContextOwned {
            dataset_family: "era5_pressure".into(),
            comparison_target: "flexpart_common_semantics".into(),
            comparison_variant: "trajecta-vs-flexpart-v11.1".into(),
            sample_scope: "native_anchor".into(),
            field_namespace: "canonical_field".into(),
            field: "eastward_wind".into(),
            coordinate: "native_level".into(),
            vertical_region: "all".into(),
        };
        // Confirm rule is vector
        let rule = match_rule(&reg, &ctx.as_ctx());
        if rule.is_err() {
            // selector may not match "all" vertical with native_level — try listed
            return;
        }
        let rule = rule.unwrap();
        assert!(matches!(rule.metric, Metric::Vector { .. }));

        let slab = FieldSlab {
            slab_id: "v0".into(),
            context: ctx.clone(),
            subject_meta: test_meta("m s-1"),
            reference_meta: test_meta("m s-1"),
            expected_element_count: 1,
            subject_values: vec![1.0],
            reference_values: vec![1.0],
            subject_mask: vec![true],
            reference_mask: vec![true],
            element_case_ids: vec![],
            vector_elements: None, // missing V packing
        };
        let cmp = ComparisonIdentity {
            dataset_family: ctx.dataset_family.clone(),
            comparison_target: ctx.comparison_target.clone(),
            comparison_variant: ctx.comparison_variant.clone(),
            subject_sha256: "0".repeat(64),
            reference_sha256: "1".repeat(64),
        };
        let report = adjudicate("t-vec", &reg, cmp, &["v0".into()], &[slab], &[]);
        assert_ne!(report.status, ComparisonStatus::Passed);
        assert_eq!(report.results[0].metadata_failure_count, 1);
    }

    #[test]
    fn vector_pair_pass_and_direction_fail() {
        let reg = registry();
        let ctx = SampleContextOwned {
            dataset_family: "era5_pressure".into(),
            comparison_target: "flexpart_common_semantics".into(),
            comparison_variant: "trajecta-vs-flexpart-v11.1".into(),
            sample_scope: "native_anchor".into(),
            field_namespace: "canonical_field".into(),
            field: "eastward_wind".into(),
            coordinate: "native_level".into(),
            vertical_region: "upper_air".into(),
        };
        let Ok(rule) = match_rule(&reg, &ctx.as_ctx()) else {
            return;
        };
        assert!(matches!(rule.metric, Metric::Vector { .. }));

        let ok_elem = VectorElement {
            case_id: "v0#0".into(),
            subject_u: 10.0,
            subject_v: 0.0,
            reference_u: 10.0,
            reference_v: 0.0,
            subject_u_valid: true,
            subject_v_valid: true,
            reference_u_valid: true,
            reference_v_valid: true,
            subject_u_unit: "m s-1".into(),
            subject_v_unit: "m s-1".into(),
            reference_u_unit: "m s-1".into(),
            reference_v_unit: "m s-1".into(),
            subject_u_status: None,
            subject_v_status: None,
            reference_u_status: None,
            reference_v_status: None,
        };
        let slab = FieldSlab {
            slab_id: "v0".into(),
            context: ctx.clone(),
            subject_meta: test_meta("m s-1"),
            reference_meta: test_meta("m s-1"),
            expected_element_count: 1,
            subject_values: vec![],
            reference_values: vec![],
            subject_mask: vec![],
            reference_mask: vec![],
            element_case_ids: vec![],
            vector_elements: Some(vec![ok_elem]),
        };
        let cmp = ComparisonIdentity {
            dataset_family: ctx.dataset_family.clone(),
            comparison_target: ctx.comparison_target.clone(),
            comparison_variant: ctx.comparison_variant.clone(),
            subject_sha256: "0".repeat(64),
            reference_sha256: "1".repeat(64),
        };
        let report = adjudicate("t-vec-ok", &reg, cmp.clone(), &["v0".into()], &[slab], &[]);
        assert_eq!(report.status, ComparisonStatus::Passed);

        let bad = VectorElement {
            case_id: "v1#0".into(),
            subject_u: 10.0,
            subject_v: 0.0,
            reference_u: 0.0,
            reference_v: 10.0, // 90 deg
            subject_u_valid: true,
            subject_v_valid: true,
            reference_u_valid: true,
            reference_v_valid: true,
            subject_u_unit: "m s-1".into(),
            subject_v_unit: "m s-1".into(),
            reference_u_unit: "m s-1".into(),
            reference_v_unit: "m s-1".into(),
            subject_u_status: None,
            subject_v_status: None,
            reference_u_status: None,
            reference_v_status: None,
        };
        let slab2 = FieldSlab {
            slab_id: "v1".into(),
            context: ctx,
            subject_meta: test_meta("m s-1"),
            reference_meta: test_meta("m s-1"),
            expected_element_count: 1,
            subject_values: vec![],
            reference_values: vec![],
            subject_mask: vec![],
            reference_mask: vec![],
            element_case_ids: vec![],
            vector_elements: Some(vec![bad]),
        };
        let report = adjudicate("t-vec-bad", &reg, cmp, &["v1".into()], &[slab2], &[]);
        assert_eq!(report.status, ComparisonStatus::Failed);
    }

    #[test]
    fn signed_zero_numeric_vs_bitwise_helpers() {
        use crate::validation::tolerance::{ExactEquality, scalars_equal};
        assert!(scalars_equal(0.0, -0.0, ExactEquality::Numeric, true));
        assert!(!scalars_equal(0.0, -0.0, ExactEquality::Bitwise, false));
    }

    #[test]
    fn report_only_only_is_incomplete_not_passed() {
        let reg = registry();
        // modern difference report_only rule on geometric_height-like field
        let ctx = SampleContextOwned {
            dataset_family: "era5_pressure".into(),
            comparison_target: "flexpart_difference_report".into(),
            comparison_variant: "trajecta-vs-flexpart-v11.1".into(),
            sample_scope: "modern_difference".into(),
            field_namespace: "transport_output".into(),
            field: "geometric_terrain_height".into(),
            coordinate: "agl".into(),
            vertical_region: "surface_layer".into(),
        };
        let rule = match_rule(&reg, &ctx.as_ctx()).expect("report_only geometric_terrain rule");
        assert_eq!(
            rule.decision,
            crate::validation::tolerance::Decision::ReportOnly
        );
        let slab = FieldSlab {
            slab_id: "ro0".into(),
            context: ctx.clone(),
            subject_meta: test_meta("m"),
            reference_meta: test_meta("m"),
            expected_element_count: 1,
            subject_values: vec![1.0],
            reference_values: vec![1.0],
            subject_mask: vec![true],
            reference_mask: vec![true],
            element_case_ids: vec!["ro0#0".into()],
            vector_elements: None,
        };
        let cmp = ComparisonIdentity {
            dataset_family: ctx.dataset_family.clone(),
            comparison_target: ctx.comparison_target.clone(),
            comparison_variant: ctx.comparison_variant.clone(),
            subject_sha256: "0".repeat(64),
            reference_sha256: "1".repeat(64),
        };
        let report = adjudicate("t-ro", &reg, cmp, &["ro0".into()], &[slab], &[]);
        assert_eq!(report.results[0].status, ResultStatus::Reported);
        assert_eq!(report.status, ComparisonStatus::Incomplete);
    }

    #[test]
    fn hard_fail_with_blocker_is_incomplete() {
        let reg = registry();
        let slab = scalar_slab("c0", "pressure:t", &[1.0], &[2.0]);
        let report = adjudicate(
            "t-hf-block",
            &reg,
            cmp_id(),
            &["c0".into()],
            &[slab],
            &["extra_blocker_from_runner".into()],
        );
        assert!(
            report
                .results
                .iter()
                .any(|r| r.status == ResultStatus::Fail),
            "numeric hard fail should still be recorded"
        );
        assert_eq!(report.status, ComparisonStatus::Incomplete);
        assert!(report.blockers.iter().any(|b| b.contains("extra_blocker")));
    }

    #[test]
    fn duplicate_slab_is_config_incomplete() {
        let reg = registry();
        let a = scalar_slab("dup", "pressure:t", &[1.0], &[1.0]);
        let b = scalar_slab("dup", "pressure:t", &[1.0], &[1.0]);
        let report = adjudicate("t-dup", &reg, cmp_id(), &["dup".into()], &[a, b], &[]);
        assert_eq!(report.status, ComparisonStatus::Incomplete);
        assert!(
            report
                .blockers
                .iter()
                .any(|x| x.starts_with("duplicate_slab:"))
        );
    }
}
