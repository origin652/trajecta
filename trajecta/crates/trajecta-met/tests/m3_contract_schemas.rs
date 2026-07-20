//! Machine-readable M3 acceptance-contract smoke tests.

use std::collections::BTreeSet;
use std::error::Error;
use std::fs;
use std::io;
use std::path::Path;

use serde_json::Value;
use sha2::{Digest, Sha256};

#[test]
fn m3_tolerance_and_oracle_schemas_are_frozen_json_documents() -> Result<(), Box<dyn Error>> {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let cases = [
        (
            "M3_TOLERANCES.schema.json",
            "https://trajecta.dev/schema/m3-tolerances-v1.json",
        ),
        (
            "M3_FLEXPART_ORACLE.schema.json",
            "https://trajecta.dev/schema/m3-flexpart-oracle-v1.json",
        ),
        (
            "M3_COMPARISON_REPORT.schema.json",
            "https://trajecta.dev/schema/m3-comparison-report-v1.json",
        ),
    ];
    for (name, expected_id) in cases {
        let value: Value =
            serde_json::from_slice(&fs::read(workspace.join("testdata").join(name))?)?;
        assert_eq!(
            value.get("$schema").and_then(Value::as_str),
            Some("https://json-schema.org/draft/2020-12/schema")
        );
        assert_eq!(value.get("$id").and_then(Value::as_str), Some(expected_id));
        assert_eq!(
            value.get("additionalProperties").and_then(Value::as_bool),
            Some(false)
        );
        assert!(value.get("$defs").and_then(Value::as_object).is_some());
    }
    Ok(())
}

fn string_array(value: &Value, key: &str) -> Result<Vec<String>, Box<dyn Error>> {
    let values = value
        .get(key)
        .and_then(Value::as_array)
        .ok_or_else(|| io::Error::other(format!("missing array {key}")))?;
    values
        .iter()
        .map(|item| {
            item.as_str()
                .map(str::to_owned)
                .ok_or_else(|| io::Error::other(format!("non-string item in {key}")).into())
        })
        .collect()
}

fn expand_all(values: Vec<String>, universe: &[&str]) -> Vec<String> {
    if values.iter().any(|value| value == "all") {
        universe.iter().map(|value| (*value).to_owned()).collect()
    } else {
        values
    }
}

#[test]
fn m3_tolerance_registry_has_frozen_evidence_and_unique_matches() -> Result<(), Box<dyn Error>> {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let registry_path = workspace.join("testdata/M3_TOLERANCES.v1.json");
    let calibration_path = workspace.join("testdata/M3_TOLERANCE_CALIBRATION.v1.json");
    let registry: Value = serde_json::from_slice(&fs::read(&registry_path)?)?;
    let calibration = fs::read(&calibration_path)?;

    assert_eq!(
        registry.get("schema_version").and_then(Value::as_str),
        Some("trajecta.m3.tolerances/v1")
    );
    assert_eq!(
        registry.get("registry_version").and_then(Value::as_str),
        Some("m3-a-tolerance/v1.0.2")
    );
    let expected_hash = registry
        .pointer("/calibration_report/sha256")
        .and_then(Value::as_str)
        .ok_or_else(|| io::Error::other("missing calibration_report.sha256"))?;
    let actual_hash = hex::encode(Sha256::digest(&calibration));
    assert_eq!(actual_hash, expected_hash, "calibration report digest");

    let rules = registry
        .get("rules")
        .and_then(Value::as_array)
        .ok_or_else(|| io::Error::other("missing tolerance rules"))?;
    let mut ids = BTreeSet::new();
    let mut expanded_matches = BTreeSet::new();
    for rule in rules {
        let id = rule
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| io::Error::other("rule without id"))?;
        assert!(ids.insert(id.to_owned()), "duplicate rule id {id}");
        if rule.get("decision").and_then(Value::as_str) == Some("report_only") {
            assert!(
                rule.get("allowlist_issue")
                    .and_then(Value::as_str)
                    .is_some(),
                "report-only rule {id} lacks allowlist_issue"
            );
        }

        let selector = rule
            .get("selector")
            .ok_or_else(|| io::Error::other(format!("rule {id} lacks selector")))?;
        let datasets = string_array(selector, "dataset_families")?;
        let fields = string_array(selector, "fields")?;
        let coordinates = expand_all(
            string_array(selector, "coordinates")?,
            &["asl", "agl", "pressure_pa", "native_level"],
        );
        let regions = expand_all(
            string_array(selector, "vertical_regions")?,
            &["surface_layer", "upper_air"],
        );
        let target = selector
            .get("comparison_target")
            .and_then(Value::as_str)
            .ok_or_else(|| io::Error::other(format!("rule {id} lacks target")))?;
        let variant = selector
            .get("comparison_variant")
            .and_then(Value::as_str)
            .ok_or_else(|| io::Error::other(format!("rule {id} lacks variant")))?;
        let scope = selector
            .get("sample_scope")
            .and_then(Value::as_str)
            .ok_or_else(|| io::Error::other(format!("rule {id} lacks scope")))?;
        let namespace = selector
            .get("field_namespace")
            .and_then(Value::as_str)
            .ok_or_else(|| io::Error::other(format!("rule {id} lacks namespace")))?;

        for dataset in &datasets {
            for field in &fields {
                for coordinate in &coordinates {
                    for region in &regions {
                        let key = format!(
                            "{dataset}|{target}|{variant}|{scope}|{namespace}|{field}|{coordinate}|{region}"
                        );
                        assert!(
                            expanded_matches.insert(key.clone()),
                            "multiple tolerance rules match {key}"
                        );
                    }
                }
            }
        }
    }

    assert!(
        ids.contains("backend/cfsr-grib/q-three-ulp/v1"),
        "CFSR q three-ULP rule must remain frozen"
    );
    assert!(
        ids.contains("backend/cfsr-grib/omega-two-ulp/v1"),
        "CFSR omega two-ULP rule must remain frozen"
    );
    let q_rule = rules
        .iter()
        .find(|rule| {
            rule.get("id").and_then(Value::as_str) == Some("backend/cfsr-grib/q-three-ulp/v1")
        })
        .ok_or_else(|| io::Error::other("missing CFSR q rule"))?;
    assert_eq!(
        string_array(
            q_rule
                .get("selector")
                .ok_or_else(|| io::Error::other("CFSR q rule lacks selector"))?,
            "fields",
        )?,
        vec!["grib:0.1.0:isobaric"],
        "q must not share its widened rule with omega"
    );
    assert_eq!(
        q_rule
            .pointer("/metric/maximum_ulps")
            .and_then(Value::as_u64),
        Some(3),
        "CFSR q release threshold"
    );
    let omega_rule = rules
        .iter()
        .find(|rule| {
            rule.get("id").and_then(Value::as_str) == Some("backend/cfsr-grib/omega-two-ulp/v1")
        })
        .ok_or_else(|| io::Error::other("missing CFSR omega rule"))?;
    assert_eq!(
        string_array(
            omega_rule
                .get("selector")
                .ok_or_else(|| io::Error::other("CFSR omega rule lacks selector"))?,
            "fields",
        )?,
        vec!["grib:0.2.8:isobaric"],
        "omega must retain its independent rule"
    );
    assert_eq!(
        omega_rule
            .pointer("/metric/maximum_ulps")
            .and_then(Value::as_u64),
        Some(2),
        "CFSR omega release threshold"
    );
    assert!(
        ids.contains("oracle/native-anchor/vector-wind/v1"),
        "FLEXPART native-anchor wind rule must remain pre-registered"
    );
    assert!(
        ids.contains("oracle/pressure-adapter/interpolated/pressure/v1"),
        "registered pressure adapter must have explicit common-semantics rules"
    );
    assert!(
        ids.contains("oracle/hybrid-native-anchor/geopotential-height/report/v1"),
        "hybrid native height algorithm difference must remain explicit"
    );
    assert!(
        ids.contains("oracle/hybrid-interpolated/pressure/asl-report/v1"),
        "hybrid ASL operator-order difference must remain explicit"
    );
    Ok(())
}
