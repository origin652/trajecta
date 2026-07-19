//! # Contract: M3 tolerance registry and comparison adjudication
//!
//! Parses frozen tolerance registries, matches rules uniquely, and builds
//! comparison reports without inventing thresholds or relaxing hard gates.
//!
#![allow(missing_docs)]

pub mod report;
pub mod tolerance;

pub use report::{
    ComparisonIdentity, ComparisonReport, ComparisonResult, ComparisonStatus, Coverage,
    FieldSample, FieldSlab, RegistryIdentity, ResultStatistics, ResultStatus, SampleContextOwned,
    SamplePair, SideMetadata, VectorElement, adjudicate, report_to_json,
};
pub use tolerance::{
    Decision, ExactEquality, ExactMetadataName, LoadedRegistry, Metric, Rule, RuleMatchError,
    SampleContext, Selector, absolute_relative_ok, direction_degrees, load_registry_from_path,
    match_rule, ordered_ulp_distance, scalars_equal, sha256_bytes, sha256_file, vector_ok,
};
