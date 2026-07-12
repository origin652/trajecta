//! Executable checks for foundational meteorology contracts.

use trajecta_met::field::{Capability, CapabilitySet};
use trajecta_met::query::request::{QueryPlanError, QueryPointArrays};

#[test]
fn capability_sets_are_explicit_and_composable() {
    let mut available = CapabilitySet::new();
    available.insert(Capability::Transport);
    available.insert(Capability::Diagnostics);

    let mut required = CapabilitySet::new();
    required.insert(Capability::Transport);
    assert!(available.contains_all(required));
}

#[test]
fn query_soa_rejects_mismatched_columns() {
    let points = QueryPointArrays {
        longitude_degrees: vec![0.0],
        latitude_degrees: Vec::new(),
        vertical: vec![100.0],
    };
    assert_eq!(points.len(), Err(QueryPlanError::LengthMismatch));
}
