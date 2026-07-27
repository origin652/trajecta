//! # Contract: exact-key observation counters for production meteorology batches
//!
//! The counters are observational. They distinguish the vectorized particle
//! loop from boundary-certificate and population helper queries so the A4
//! `<= 13` contract measures the intended bulk transport path without hiding
//! the cost of internal scalar work.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

use sha2::{Digest, Sha256};

use trajecta_case::model::meteorology::DomainId;
use trajecta_case::model::time::Timestamp;

use crate::query::request::{QueryBatch, QueryPlan, VerticalQuery};

thread_local! {
    static ACTIVE_QUERY_COUNTERS: RefCell<Option<Arc<QueryCallCounters>>> = const { RefCell::new(None) };
    static ACTIVE_QUERY_ORIGIN: RefCell<QueryOrigin> = const { RefCell::new(QueryOrigin::Unclassified) };
}

/// Stable caller category for one prepared query.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum QueryOrigin {
    /// Caller did not install a more precise scope.
    Unclassified,
    /// Vectorized RK2 particle query.
    Integrator,
    /// Vectorized particle-state output query.
    Output,
    /// Birth/termination-only particle-state output query.
    ///
    /// Lifecycle deltas are disclosed separately because their exact times and
    /// batch sizes depend on physical births/terminations; they are not part of
    /// the frozen six-step Integrator + scheduled Output bulk-query gate.
    OutputLifecycle,
    /// Continuous boundary-path certification or root sampling.
    Boundary,
    /// Four-corner terrain/model-top probes used by boundary certification.
    BoundaryCorners,
    /// Population vertical-coordinate helper.
    PopulationVertical,
}

impl QueryOrigin {
    /// Stable machine label.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::Unclassified => "unclassified",
            Self::Integrator => "integrator",
            Self::Output => "output",
            Self::OutputLifecycle => "output_lifecycle",
            Self::Boundary => "boundary",
            Self::BoundaryCorners => "boundary_corners",
            Self::PopulationVertical => "population_vertical",
        }
    }

    const fn is_particle_loop(self) -> bool {
        matches!(self, Self::Integrator | Self::Output)
    }

    /// Whether a completed request may replace the engine's bulk transport
    /// cache entry.
    ///
    /// Lifecycle/helper queries may still consume an exact cached result, but
    /// admitting their typically tiny, one-off batches would evict the full
    /// scheduled output needed by the next macro-step integrator start.
    pub(crate) const fn populates_transport_cache(self) -> bool {
        matches!(self, Self::Unclassified | Self::Integrator | Self::Output)
    }
}

/// Restores the previous query origin when a caller scope ends.
pub struct QueryOriginScope {
    previous: QueryOrigin,
}

impl QueryOriginScope {
    /// Enters one caller category for the current thread.
    #[must_use]
    pub fn enter(origin: QueryOrigin) -> Self {
        let previous = ACTIVE_QUERY_ORIGIN.with(|slot| {
            let previous = *slot.borrow();
            *slot.borrow_mut() = origin;
            previous
        });
        Self { previous }
    }
}

impl Drop for QueryOriginScope {
    fn drop(&mut self) {
        ACTIVE_QUERY_ORIGIN.with(|slot| *slot.borrow_mut() = self.previous);
    }
}

/// Installs exact-key observation counters for the current thread.
pub fn install_query_counters(counters: Arc<QueryCallCounters>) {
    ACTIVE_QUERY_COUNTERS.with(|slot| *slot.borrow_mut() = Some(counters));
}

/// Clears the current-thread exact-key observation counters.
pub fn clear_query_counters() {
    ACTIVE_QUERY_COUNTERS.with(|slot| *slot.borrow_mut() = None);
}

/// Returns the current-thread query counters, when installed.
#[must_use]
pub fn active_query_counters() -> Option<Arc<QueryCallCounters>> {
    ACTIVE_QUERY_COUNTERS.with(|slot| slot.borrow().clone())
}

/// Returns the current caller category.
#[must_use]
pub fn active_query_origin() -> QueryOrigin {
    ACTIVE_QUERY_ORIGIN.with(|slot| *slot.borrow())
}

/// Exact digest and caller category retained by a prepared batch.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExactQueryKey {
    digest: [u8; 32],
    origin: QueryOrigin,
}

impl ExactQueryKey {
    /// Hex-encoded SHA-256 identity.
    #[must_use]
    pub fn sha256(&self) -> String {
        hex::encode(self.digest)
    }
}

/// Atomic/shareable query counters backed by one bounded mutex state.
#[derive(Debug, Default)]
pub struct QueryCallCounters {
    state: Mutex<CounterState>,
}

#[derive(Debug, Default)]
struct CounterState {
    by_origin: BTreeMap<QueryOrigin, OriginCounterState>,
    particle_loop_exact: ExactCounterState,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct OriginCounterState {
    logical_requests: u64,
    executed_batches: u64,
    exact_key_reuses: u64,
}

#[derive(Debug, Default)]
struct ExactCounterState {
    executed_keys: BTreeSet<[u8; 32]>,
    repeated_exact_executions: u64,
    first_repeated_executed_key: Option<[u8; 32]>,
}

/// Immutable per-origin counts.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct QueryOriginSnapshot {
    /// Logical requests prepared by this caller category.
    pub logical_requests: u64,
    /// Batches that performed actual interpolation work.
    pub executed_batches: u64,
    /// Requests served by the production exact transport cache.
    pub exact_key_reuses: u64,
}

/// Exact-key audit for the vectorized integrator/output particle loop.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ParticleLoopExactSnapshot {
    /// Distinct exact bulk request keys.
    pub unique_exact_keys: u64,
    /// Exact keys that performed interpolation more than once.
    pub repeated_exact_executions: u64,
    /// First repeated executed key, if any.
    pub first_repeated_executed_key: Option<String>,
}

/// Immutable snapshot of query execution observation.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct QueryCallSnapshot {
    /// Aggregate logical requests across all caller categories.
    pub logical_requests: u64,
    /// Aggregate batches that performed interpolation.
    pub executed_batches: u64,
    /// Aggregate exact-cache reuses.
    pub exact_key_reuses: u64,
    /// Exact audit limited to the bounded vectorized particle loop.
    pub particle_loop_exact: ParticleLoopExactSnapshot,
    /// Per-caller request/execution/reuse counts.
    pub by_origin: BTreeMap<QueryOrigin, QueryOriginSnapshot>,
}

impl QueryCallCounters {
    /// Creates an empty, shareable production counter set.
    #[must_use]
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Records a logical request and returns its exact SHA-256 identity.
    pub fn record_logical_request(
        &self,
        origin: QueryOrigin,
        time: Timestamp,
        domain: &DomainId,
        plan: &QueryPlan,
        batch: &QueryBatch,
    ) -> ExactQueryKey {
        // Only the frozen Integrator + Output gate audits exact identities.
        // Boundary and helper origins disclose counts but never retain or
        // compare their keys, so hashing the complete plan and coordinates for
        // tens of thousands of scalar probes would be pure observer overhead.
        let digest = if origin.is_particle_loop() {
            exact_key_sha256(time, domain, plan, batch)
        } else {
            [0; 32]
        };
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let origin_state = state.by_origin.entry(origin).or_default();
        origin_state.logical_requests = origin_state.logical_requests.saturating_add(1);
        ExactQueryKey { digest, origin }
    }

    /// Records that interpolation work actually executed for a prepared batch.
    pub fn record_execution(&self, key: ExactQueryKey) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let origin_state = state.by_origin.entry(key.origin).or_default();
        origin_state.executed_batches = origin_state.executed_batches.saturating_add(1);
        if key.origin.is_particle_loop()
            && !state.particle_loop_exact.executed_keys.insert(key.digest)
        {
            state.particle_loop_exact.repeated_exact_executions = state
                .particle_loop_exact
                .repeated_exact_executions
                .saturating_add(1);
            if state
                .particle_loop_exact
                .first_repeated_executed_key
                .is_none()
            {
                state.particle_loop_exact.first_repeated_executed_key = Some(key.digest);
            }
        }
    }

    /// Records a production cache hit for one exact prepared request.
    pub fn record_exact_key_reuse(&self, key: ExactQueryKey) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let origin_state = state.by_origin.entry(key.origin).or_default();
        origin_state.exact_key_reuses = origin_state.exact_key_reuses.saturating_add(1);
    }

    /// Takes an immutable snapshot without changing execution behavior.
    #[must_use]
    pub fn snapshot(&self) -> QueryCallSnapshot {
        let state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let by_origin = state
            .by_origin
            .iter()
            .map(|(origin, counts)| {
                (
                    *origin,
                    QueryOriginSnapshot {
                        logical_requests: counts.logical_requests,
                        executed_batches: counts.executed_batches,
                        exact_key_reuses: counts.exact_key_reuses,
                    },
                )
            })
            .collect::<BTreeMap<_, _>>();
        QueryCallSnapshot {
            logical_requests: by_origin
                .values()
                .map(|counts| counts.logical_requests)
                .sum(),
            executed_batches: by_origin
                .values()
                .map(|counts| counts.executed_batches)
                .sum(),
            exact_key_reuses: by_origin
                .values()
                .map(|counts| counts.exact_key_reuses)
                .sum(),
            particle_loop_exact: ParticleLoopExactSnapshot {
                unique_exact_keys: u64::try_from(state.particle_loop_exact.executed_keys.len())
                    .unwrap_or(u64::MAX),
                repeated_exact_executions: state.particle_loop_exact.repeated_exact_executions,
                first_repeated_executed_key: state
                    .particle_loop_exact
                    .first_repeated_executed_key
                    .map(hex::encode),
            },
            by_origin,
        }
    }
}

/// Stable SHA-256 identity for one exact production batch request.
///
/// The byte stream includes timestamp, domain, the complete compiled plan
/// identity relevant to results, vertical-coordinate kind, and every input
/// coordinate IEEE-754 bit pattern in caller order.
#[must_use]
pub fn exact_key_sha256(
    time: Timestamp,
    domain: &DomainId,
    plan: &QueryPlan,
    batch: &QueryBatch,
) -> [u8; 32] {
    let mut hash = Sha256::new();
    hash.update(b"trajecta.exact-batch-query-key/v2\0");
    hash.update(time.seconds_since_unix_epoch().to_le_bytes());
    hash.update(time.nanosecond().to_le_bytes());
    put_bytes(&mut hash, domain.0.as_bytes());
    hash.update([vertical_tag(batch.vertical_coordinate)]);
    hash.update([u8::from(plan.allow_estimated())]);
    hash.update([match plan.explain_mode() {
        crate::query::request::ExplainMode::Disabled => 0,
        crate::query::request::ExplainMode::Full => 1,
    }]);
    put_bytes(&mut hash, format!("{:?}", plan.capabilities()).as_bytes());
    put_bytes(
        &mut hash,
        plan.surface_layer_model()
            .map_or(b"", |model| model.0.as_bytes()),
    );
    put_bytes(&mut hash, format!("{:?}", plan.derivations()).as_bytes());
    hash.update(
        u64::try_from(plan.fields().len())
            .unwrap_or(u64::MAX)
            .to_le_bytes(),
    );
    for field in plan.fields() {
        put_bytes(&mut hash, format!("{field:?}").as_bytes());
    }
    let len = batch.points.len().unwrap_or(usize::MAX);
    hash.update(u64::try_from(len).unwrap_or(u64::MAX).to_le_bytes());
    for index in 0..len {
        hash.update(
            batch.points.longitude_degrees[index]
                .to_bits()
                .to_le_bytes(),
        );
        hash.update(batch.points.latitude_degrees[index].to_bits().to_le_bytes());
        hash.update(batch.points.vertical[index].to_bits().to_le_bytes());
    }
    hash.finalize().into()
}

fn put_bytes(hash: &mut Sha256, value: &[u8]) {
    hash.update(u64::try_from(value.len()).unwrap_or(u64::MAX).to_le_bytes());
    hash.update(value);
}

const fn vertical_tag(value: VerticalQuery) -> u8 {
    match value {
        VerticalQuery::AboveSeaLevel => 1,
        VerticalQuery::AboveGround => 2,
        VerticalQuery::Pressure => 3,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn origin_scope_restores_previous_value() {
        assert_eq!(active_query_origin(), QueryOrigin::Unclassified);
        {
            let _scope = QueryOriginScope::enter(QueryOrigin::Integrator);
            assert_eq!(active_query_origin(), QueryOrigin::Integrator);
        }
        assert_eq!(active_query_origin(), QueryOrigin::Unclassified);
    }

    #[test]
    fn particle_loop_exact_tracking_is_bounded_to_bulk_origins() {
        let counters = QueryCallCounters::new();
        let key = ExactQueryKey {
            digest: [7; 32],
            origin: QueryOrigin::Integrator,
        };
        counters.record_execution(key);
        counters.record_execution(key);
        counters.record_exact_key_reuse(ExactQueryKey {
            digest: [8; 32],
            origin: QueryOrigin::Output,
        });
        let boundary = ExactQueryKey {
            digest: [9; 32],
            origin: QueryOrigin::Boundary,
        };
        counters.record_execution(boundary);
        counters.record_execution(boundary);
        let snapshot = counters.snapshot();
        assert_eq!(snapshot.particle_loop_exact.repeated_exact_executions, 1);
        assert_eq!(snapshot.exact_key_reuses, 1);
        assert_eq!(
            snapshot.by_origin[&QueryOrigin::Boundary].executed_batches,
            2
        );
    }
}
