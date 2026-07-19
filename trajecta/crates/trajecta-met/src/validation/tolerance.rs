//! # Contract: tolerance registry loading, selectors, and metrics
//!
//! Implements exact/ULP/absolute-relative/vector metrics and unique rule
//! matching for the frozen M3 A-tolerance contract.
//!
use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use sha2::{Digest, Sha256};

/// Decision class for one matched rule.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    /// Failure makes the report fail.
    HardGate,
    /// Failure is recorded without flipping hard-gate overall status.
    ReportOnly,
}

/// Exact equality flavour for the `exact` metric.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExactEquality {
    /// Finite numeric equality (`+0`/`-0` may be equal by policy).
    Numeric,
    /// Bit-identical IEEE representation.
    Bitwise,
}

/// Named metadata that must match exactly before numeric metrics run.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExactMetadataName {
    ValidTimes,
    Grid,
    VerticalTopology,
    Layout,
    Mask,
    Unit,
    TemporalSupport,
    Status,
    Validity,
}

/// Numeric comparison metric.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Metric {
    /// Exact scalar equality.
    Exact {
        equality: ExactEquality,
        signed_zero_equal: bool,
    },
    /// Ordered binary64 ULP distance.
    Ulp {
        maximum_ulps: u64,
        signed_zero_equal: bool,
    },
    /// Absolute + relative envelope.
    AbsoluteRelative {
        absolute: f64,
        relative: f64,
        scale_floor: f64,
    },
    /// Paired U/V components with conditional direction gate.
    Vector {
        component_absolute: f64,
        component_relative: f64,
        component_scale_floor: f64,
        speed_floor: f64,
        direction_degrees: f64,
    },
}

/// Rule selector fields (all required; unknown keys rejected).
#[derive(Clone, Debug, Eq, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Selector {
    pub dataset_families: Vec<String>,
    pub comparison_target: String,
    pub comparison_variant: String,
    pub sample_scope: String,
    pub field_namespace: String,
    pub fields: Vec<String>,
    pub coordinates: Vec<String>,
    pub vertical_regions: Vec<String>,
}

/// One frozen comparison rule.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rule {
    pub id: String,
    pub selector: Selector,
    pub decision: Decision,
    pub exact_metadata: Vec<ExactMetadataName>,
    pub mask_policy: String,
    pub non_finite_policy: String,
    pub metric: Metric,
    pub threshold_basis: String,
    pub evidence_ids: Vec<String>,
    pub scientific_reason: String,
    #[serde(default)]
    pub allowlist_issue: Option<String>,
}

/// Calibration report pointer inside the registry.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CalibrationReportRef {
    pub path: String,
    pub sha256: String,
    pub state: String,
}

/// Root document for `M3_TOLERANCES.v1.json`.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToleranceRegistryDocument {
    pub schema_version: String,
    pub algorithm_id: String,
    pub registry_version: String,
    pub approved_at_utc: String,
    pub approval_authority: String,
    pub calibration_report: CalibrationReportRef,
    pub rules: Vec<Rule>,
}

/// Loaded registry with integrity hash.
#[derive(Clone, Debug, PartialEq)]
pub struct LoadedRegistry {
    /// Document body.
    pub document: ToleranceRegistryDocument,
    /// SHA-256 of the registry file bytes.
    pub sha256: String,
    /// Absolute path the registry was loaded from.
    pub path: PathBuf,
}

/// Sample context used for unique rule matching.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SampleContext<'a> {
    pub dataset_family: &'a str,
    pub comparison_target: &'a str,
    pub comparison_variant: &'a str,
    pub sample_scope: &'a str,
    pub field_namespace: &'a str,
    pub field: &'a str,
    pub coordinate: &'a str,
    pub vertical_region: &'a str,
}

/// Rule match failure modes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RuleMatchError {
    /// Zero rules matched after selector expansion.
    Unvalidated,
    /// More than one rule matched.
    Ambiguous,
}

/// SHA-256 hex of file bytes.
pub fn sha256_file(path: &Path) -> Result<String, String> {
    let bytes = fs::read(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    Ok(sha256_bytes(&bytes))
}

/// SHA-256 hex of an in-memory buffer.
#[must_use]
pub fn sha256_bytes(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// Load and structurally validate a frozen registry file.
pub fn load_registry_from_path(path: &Path) -> Result<LoadedRegistry, String> {
    let bytes = fs::read(path).map_err(|e| format!("read registry {}: {e}", path.display()))?;
    let sha = sha256_bytes(&bytes);
    let document: ToleranceRegistryDocument = serde_json::from_slice(&bytes)
        .map_err(|e| format!("parse registry {}: {e}", path.display()))?;
    validate_registry_document(&document, path)?;
    // Calibration path/SHA must resolve relative to workspace root (registry parent/parent)
    // or relative to the registry file's directory, then repo root.
    validate_calibration_pointer(&document.calibration_report, path)?;
    Ok(LoadedRegistry {
        document,
        sha256: sha,
        path: path.to_path_buf(),
    })
}

fn validate_registry_document(doc: &ToleranceRegistryDocument, path: &Path) -> Result<(), String> {
    if doc.schema_version != "trajecta.m3.tolerances/v1" {
        return Err(format!(
            "unsupported schema_version in {}: {}",
            path.display(),
            doc.schema_version
        ));
    }
    if doc.algorithm_id != "trajecta/met_query/m3/v0" {
        return Err(format!(
            "unsupported algorithm_id in {}: {}",
            path.display(),
            doc.algorithm_id
        ));
    }
    if doc.approval_authority != "A" {
        return Err(format!(
            "approval_authority must be A in {}",
            path.display()
        ));
    }
    if !matches!(
        doc.calibration_report.state.as_str(),
        "measured_partial" | "measured_complete"
    ) {
        return Err(format!(
            "unknown calibration state {}",
            doc.calibration_report.state
        ));
    }
    if doc.rules.is_empty() {
        return Err("registry has no rules".into());
    }
    for rule in &doc.rules {
        if rule.mask_policy != "exact" {
            return Err(format!("rule {} mask_policy must be exact", rule.id));
        }
        if rule.non_finite_policy != "reject" {
            return Err(format!("rule {} non_finite_policy must be reject", rule.id));
        }
        if rule.exact_metadata.is_empty() {
            return Err(format!("rule {} exact_metadata empty", rule.id));
        }
        if rule.scientific_reason.len() < 30 {
            return Err(format!("rule {} scientific_reason too short", rule.id));
        }
        match rule.decision {
            Decision::ReportOnly if rule.allowlist_issue.as_ref().is_none_or(String::is_empty) => {
                return Err(format!(
                    "rule {} report_only requires allowlist_issue",
                    rule.id
                ));
            }
            Decision::HardGate if rule.allowlist_issue.is_some() => {
                return Err(format!(
                    "rule {} hard_gate must not carry allowlist_issue",
                    rule.id
                ));
            }
            _ => {}
        }
        match &rule.metric {
            Metric::AbsoluteRelative {
                absolute,
                relative,
                scale_floor,
            } => {
                if *absolute < 0.0 || *relative < 0.0 || *scale_floor <= 0.0 {
                    return Err(format!("rule {} abs-rel thresholds invalid", rule.id));
                }
            }
            Metric::Vector {
                component_absolute,
                component_relative,
                component_scale_floor,
                speed_floor,
                direction_degrees,
            } => {
                if *component_absolute < 0.0
                    || *component_relative < 0.0
                    || *component_scale_floor <= 0.0
                    || *speed_floor <= 0.0
                    || !(0.0..=180.0).contains(direction_degrees)
                {
                    return Err(format!("rule {} vector thresholds invalid", rule.id));
                }
            }
            Metric::Ulp { .. } | Metric::Exact { .. } => {}
        }
    }
    Ok(())
}

fn validate_calibration_pointer(
    cal: &CalibrationReportRef,
    registry_path: &Path,
) -> Result<(), String> {
    if cal.sha256.len() != 64 || !cal.sha256.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(format!("calibration sha256 malformed: {}", cal.sha256));
    }
    let candidates = [
        registry_path
            .parent()
            .map(|p| p.join(&cal.path))
            .unwrap_or_else(|| PathBuf::from(&cal.path)),
        registry_path
            .parent()
            .and_then(|p| p.parent())
            .map(|p| p.join(&cal.path))
            .unwrap_or_else(|| PathBuf::from(&cal.path)),
        PathBuf::from(&cal.path),
    ];
    let mut found = None;
    for cand in candidates {
        if cand.is_file() {
            found = Some(cand);
            break;
        }
    }
    let path = found.ok_or_else(|| {
        format!(
            "calibration file missing for path {} (searched relative to registry)",
            cal.path
        )
    })?;
    let actual = sha256_file(&path)?;
    if actual != cal.sha256 {
        return Err(format!(
            "calibration SHA mismatch for {}: expected {}, got {}",
            path.display(),
            cal.sha256,
            actual
        ));
    }
    Ok(())
}

fn list_matches(items: &[String], value: &str) -> bool {
    items.iter().any(|item| item == "all" || item == value)
}

/// Unique rule match against a sample context.
pub fn match_rule<'a>(
    registry: &'a LoadedRegistry,
    ctx: &SampleContext<'_>,
) -> Result<&'a Rule, RuleMatchError> {
    let mut hits: Vec<&Rule> = Vec::new();
    for rule in &registry.document.rules {
        let sel = &rule.selector;
        if !sel.dataset_families.iter().any(|f| f == ctx.dataset_family) {
            continue;
        }
        if sel.comparison_target != ctx.comparison_target {
            continue;
        }
        if sel.comparison_variant != ctx.comparison_variant {
            continue;
        }
        if sel.sample_scope != ctx.sample_scope {
            continue;
        }
        if sel.field_namespace != ctx.field_namespace {
            continue;
        }
        if !sel.fields.iter().any(|f| f == ctx.field) {
            continue;
        }
        if !list_matches(&sel.coordinates, ctx.coordinate) {
            continue;
        }
        if !list_matches(&sel.vertical_regions, ctx.vertical_region) {
            continue;
        }
        hits.push(rule);
    }
    match hits.as_slice() {
        [] => Err(RuleMatchError::Unvalidated),
        [one] => Ok(*one),
        _ => Err(RuleMatchError::Ambiguous),
    }
}

/// Map finite binary64 to an ordered unsigned integer for ULP distance.
#[must_use]
pub fn ordered_binary64_key(value: f64) -> u64 {
    let bits = value.to_bits();
    if bits & (1_u64 << 63) == 0 {
        bits | (1_u64 << 63)
    } else {
        !bits
    }
}

/// Ordered binary64 ULP distance between two finite values.
#[must_use]
pub fn ordered_ulp_distance(a: f64, b: f64) -> u64 {
    debug_assert!(a.is_finite() && b.is_finite());
    let ka = ordered_binary64_key(a);
    let kb = ordered_binary64_key(b);
    ka.abs_diff(kb)
}

/// Exact scalar equality under the declared policy.
#[must_use]
pub fn scalars_equal(a: f64, b: f64, equality: ExactEquality, signed_zero_equal: bool) -> bool {
    match equality {
        ExactEquality::Numeric => {
            if signed_zero_equal && a == 0.0 && b == 0.0 {
                return true;
            }
            a == b
        }
        ExactEquality::Bitwise => {
            if signed_zero_equal && a == 0.0 && b == 0.0 {
                return true;
            }
            a.to_bits() == b.to_bits()
        }
    }
}

/// Absolute-relative envelope check.
#[must_use]
pub fn absolute_relative_ok(
    subject: f64,
    reference: f64,
    absolute: f64,
    relative: f64,
    scale_floor: f64,
) -> bool {
    if !subject.is_finite() || !reference.is_finite() {
        return false;
    }
    let scale = subject.abs().max(reference.abs()).max(scale_floor);
    (subject - reference).abs() <= absolute + relative * scale
}

/// Shortest direction difference in degrees for two (u,v) pairs.
#[must_use]
pub fn direction_degrees(su: f64, sv: f64, ru: f64, rv: f64) -> f64 {
    let a = sv.atan2(su).to_degrees();
    let b = rv.atan2(ru).to_degrees();
    let mut d = (a - b).abs();
    if d > 180.0 {
        d = 360.0 - d;
    }
    d
}

/// Vector gate: component abs-rel plus conditional direction.
#[must_use]
#[allow(clippy::too_many_arguments)]
pub fn vector_ok(
    su: f64,
    sv: f64,
    ru: f64,
    rv: f64,
    component_absolute: f64,
    component_relative: f64,
    component_scale_floor: f64,
    speed_floor: f64,
    direction_max: f64,
) -> bool {
    if !su.is_finite() || !sv.is_finite() || !ru.is_finite() || !rv.is_finite() {
        return false;
    }
    if !absolute_relative_ok(
        su,
        ru,
        component_absolute,
        component_relative,
        component_scale_floor,
    ) || !absolute_relative_ok(
        sv,
        rv,
        component_absolute,
        component_relative,
        component_scale_floor,
    ) {
        return false;
    }
    let speed_s = (su * su + sv * sv).sqrt();
    let speed_r = (ru * ru + rv * rv).sqrt();
    if speed_s >= speed_floor && speed_r >= speed_floor {
        return direction_degrees(su, sv, ru, rv) <= direction_max;
    }
    true
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn registry_path() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../testdata/M3_TOLERANCES.v1.json")
    }

    #[test]
    fn loads_frozen_registry_and_checks_calibration_sha() {
        let reg = load_registry_from_path(&registry_path()).expect("load");
        assert_eq!(reg.document.registry_version, "m3-a-tolerance/v1.0.1");
        assert!(!reg.sha256.is_empty());
    }

    #[test]
    fn unique_match_and_unvalidated() {
        let reg = load_registry_from_path(&registry_path()).unwrap();
        let hit = match_rule(
            &reg,
            &SampleContext {
                dataset_family: "era5_pressure",
                comparison_target: "backend_equivalence",
                comparison_variant: "rust-netcdf-vs-native-netcdf",
                sample_scope: "decoded_field",
                field_namespace: "source_identity",
                field: "pressure:t",
                coordinate: "asl",
                vertical_region: "upper_air",
            },
        )
        .unwrap();
        assert_eq!(hit.id, "backend/era5-pressure/netcdf-exact/v1");
        let err = match_rule(
            &reg,
            &SampleContext {
                dataset_family: "analytic",
                comparison_target: "backend_equivalence",
                comparison_variant: "no-such",
                sample_scope: "decoded_field",
                field_namespace: "source_identity",
                field: "t",
                coordinate: "asl",
                vertical_region: "upper_air",
            },
        )
        .unwrap_err();
        assert_eq!(err, RuleMatchError::Unvalidated);
    }

    #[test]
    fn signed_zero_and_ulp_boundaries() {
        assert!(scalars_equal(0.0, -0.0, ExactEquality::Numeric, true));
        assert!(!scalars_equal(0.0, -0.0, ExactEquality::Bitwise, false));
        let a = 1.0_f64;
        let b1 = f64::from_bits(a.to_bits() + 1);
        let b2 = f64::from_bits(a.to_bits() + 2);
        let b3 = f64::from_bits(a.to_bits() + 3);
        assert_eq!(ordered_ulp_distance(a, b1), 1);
        assert_eq!(ordered_ulp_distance(a, b2), 2);
        assert_eq!(ordered_ulp_distance(a, b3), 3);
        assert!(absolute_relative_ok(1.0, 1.0 + 1e-9, 0.0, 1e-6, 1.0));
        assert!(!absolute_relative_ok(1.0, 2.0, 0.0, 1e-6, 1.0));
    }

    #[test]
    fn reject_unknown_fields_in_registry_blob() {
        let bad = br#"{"schema_version":"trajecta.m3.tolerances/v1","algorithm_id":"trajecta/met_query/m3/v0","registry_version":"x","approved_at_utc":"2026-01-01T00:00:00Z","approval_authority":"A","calibration_report":{"path":"p","sha256":"00","state":"measured_partial"},"rules":[],"extra":1}"#;
        let err = serde_json::from_slice::<ToleranceRegistryDocument>(bad).unwrap_err();
        assert!(err.to_string().contains("unknown field") || err.to_string().contains("extra"));
    }
}
