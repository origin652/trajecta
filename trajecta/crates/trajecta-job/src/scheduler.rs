//! # Contract: deterministic local resource scheduling
//!
//! The scheduler is deliberately pure. It observes durable FIFO queue rows and
//! current reservations, then returns an atomic dispatch plan for the catalog
//! to apply. It never starts or stops a worker itself.

use std::collections::BTreeMap;

use trajecta_core::manifest::RunId;

use crate::model::ResourceRequest;

/// Frozen maximum number of times a blocked FIFO head may be bypassed.
pub const MAXIMUM_HEAD_BYPASS: u8 = 3;

/// Total resources available to the local daemon after its reserve is removed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SchedulerCapacity {
    /// Available logical CPU slots.
    pub cpu_slots: u32,
    /// Available memory reservations in mebibytes.
    pub memory_mib: u64,
}

impl SchedulerCapacity {
    /// Validates a non-empty scheduling pool.
    pub fn validate(self) -> Result<(), SchedulerError> {
        if self.cpu_slots == 0 || self.memory_mib == 0 {
            return Err(SchedulerError::InvalidCapacity);
        }
        Ok(())
    }
}

/// Resources already reserved by starting or running workers.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ResourceUsage {
    /// Reserved logical CPU slots.
    pub cpu_slots: u32,
    /// Reserved memory in mebibytes.
    pub memory_mib: u64,
}

impl ResourceUsage {
    fn fits(self, request: ResourceRequest) -> bool {
        request.cpu_slots <= self.cpu_slots && request.memory_mib <= self.memory_mib
    }

    fn consume(&mut self, request: ResourceRequest) -> Result<(), SchedulerError> {
        self.cpu_slots = self
            .cpu_slots
            .checked_sub(request.cpu_slots)
            .ok_or(SchedulerError::InvalidActiveUsage)?;
        self.memory_mib = self
            .memory_mib
            .checked_sub(request.memory_mib)
            .ok_or(SchedulerError::InvalidActiveUsage)?;
        Ok(())
    }
}

/// One durable queued attempt supplied in FIFO order.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QueuedAttempt {
    /// Unique attempt identity.
    pub run_id: RunId,
    /// Explicit resource reservation.
    pub resources: ResourceRequest,
    /// Number of prior safe-backfill overtakes while this row was queue head.
    pub head_bypass_count: u8,
}

/// Updated bypass count that must be persisted with a dispatch plan.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BypassUpdate {
    /// Blocked queue-head attempt.
    pub run_id: RunId,
    /// New durable bypass count.
    pub head_bypass_count: u8,
}

/// Why a scheduling pass stopped before exhausting the queue.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DispatchStop {
    /// An external memory-pressure signal paused all new dispatch.
    ExternalMemoryPressure,
    /// Available reservations could not fit any legal next job.
    InsufficientResources,
    /// The FIFO head reached its bypass cap, so resources must be reserved for it.
    HeadReservation,
}

/// Deterministic set of rows to reserve in one catalog transaction.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DispatchPlan {
    /// Attempts to transition from queued to starting, in dispatch order.
    pub selected_run_ids: Vec<RunId>,
    /// Queue-head bypass counters to update atomically with selection.
    pub bypass_updates: Vec<BypassUpdate>,
    /// Optional reason the pass stopped with queued work remaining.
    pub stopped_by: Option<DispatchStop>,
}

/// Pure scheduler validation failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SchedulerError {
    /// Total capacity must contain positive CPU and memory resources.
    InvalidCapacity,
    /// Active reservations exceed configured capacity.
    InvalidActiveUsage,
    /// A queued request is internally invalid.
    InvalidRequest(RunId),
    /// A request can never fit the configured daemon capacity.
    RequestExceedsCapacity(RunId),
    /// A persisted bypass counter exceeds the frozen cap.
    InvalidBypassCount(RunId),
}

/// Plans one FIFO/safe-backfill scheduling pass.
///
/// Every selected backfill counts as one overtake of the blocked head. Once the
/// head reaches [`MAXIMUM_HEAD_BYPASS`], no younger row may be dispatched until
/// that head can start. External pressure always returns an empty plan and does
/// not alter existing reservations.
pub fn plan_dispatch(
    capacity: SchedulerCapacity,
    active: ResourceUsage,
    external_memory_pressure: bool,
    queue: &[QueuedAttempt],
) -> Result<DispatchPlan, SchedulerError> {
    capacity.validate()?;
    if active.cpu_slots > capacity.cpu_slots || active.memory_mib > capacity.memory_mib {
        return Err(SchedulerError::InvalidActiveUsage);
    }
    for attempt in queue {
        if attempt.resources.validate().is_err() {
            return Err(SchedulerError::InvalidRequest(attempt.run_id.clone()));
        }
        if attempt.resources.cpu_slots > capacity.cpu_slots
            || attempt.resources.memory_mib > capacity.memory_mib
        {
            return Err(SchedulerError::RequestExceedsCapacity(
                attempt.run_id.clone(),
            ));
        }
        if attempt.head_bypass_count > MAXIMUM_HEAD_BYPASS {
            return Err(SchedulerError::InvalidBypassCount(attempt.run_id.clone()));
        }
    }

    if external_memory_pressure {
        return Ok(DispatchPlan {
            stopped_by: (!queue.is_empty()).then_some(DispatchStop::ExternalMemoryPressure),
            ..DispatchPlan::default()
        });
    }

    let mut available = ResourceUsage {
        cpu_slots: capacity.cpu_slots - active.cpu_slots,
        memory_mib: capacity.memory_mib - active.memory_mib,
    };
    let mut remaining = queue.to_vec();
    let mut selected_run_ids = Vec::new();
    let mut bypass_updates = BTreeMap::<RunId, u8>::new();
    let mut stopped_by = None;

    while let Some(head) = remaining.first() {
        if available.fits(head.resources) {
            let selected = remaining.remove(0);
            available.consume(selected.resources)?;
            selected_run_ids.push(selected.run_id);
            continue;
        }

        if head.head_bypass_count >= MAXIMUM_HEAD_BYPASS {
            stopped_by = Some(DispatchStop::HeadReservation);
            break;
        }

        let backfill_index = remaining
            .iter()
            .enumerate()
            .skip(1)
            .find_map(|(index, candidate)| available.fits(candidate.resources).then_some(index));
        let Some(backfill_index) = backfill_index else {
            stopped_by = Some(DispatchStop::InsufficientResources);
            break;
        };

        let next_bypass = head.head_bypass_count + 1;
        let head_run_id = head.run_id.clone();
        remaining[0].head_bypass_count = next_bypass;
        bypass_updates.insert(head_run_id, next_bypass);

        let selected = remaining.remove(backfill_index);
        available.consume(selected.resources)?;
        selected_run_ids.push(selected.run_id);
    }

    Ok(DispatchPlan {
        selected_run_ids,
        bypass_updates: bypass_updates
            .into_iter()
            .map(|(run_id, head_bypass_count)| BypassUpdate {
                run_id,
                head_bypass_count,
            })
            .collect(),
        stopped_by,
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn run(value: u8) -> RunId {
        RunId(format!("018f0000-0000-7000-8000-{value:012x}"))
    }

    fn queued(value: u8, cpu_slots: u32, memory_mib: u64, bypass: u8) -> QueuedAttempt {
        QueuedAttempt {
            run_id: run(value),
            resources: ResourceRequest {
                cpu_slots,
                memory_mib,
                worker_threads: cpu_slots,
            },
            head_bypass_count: bypass,
        }
    }

    fn capacity() -> SchedulerCapacity {
        SchedulerCapacity {
            cpu_slots: 8,
            memory_mib: 8_192,
        }
    }

    #[test]
    fn fifo_jobs_dispatch_in_order_when_they_fit() {
        let plan = plan_dispatch(
            capacity(),
            ResourceUsage::default(),
            false,
            &[queued(1, 4, 2_048, 0), queued(2, 4, 2_048, 0)],
        )
        .unwrap();
        assert_eq!(plan.selected_run_ids, vec![run(1), run(2)]);
        assert!(plan.bypass_updates.is_empty());
        assert_eq!(plan.stopped_by, None);
    }

    #[test]
    fn safe_backfill_stops_after_three_head_overtakes() {
        let plan = plan_dispatch(
            capacity(),
            ResourceUsage {
                cpu_slots: 4,
                memory_mib: 0,
            },
            false,
            &[
                queued(1, 8, 4_096, 0),
                queued(2, 1, 512, 0),
                queued(3, 1, 512, 0),
                queued(4, 1, 512, 0),
                queued(5, 1, 512, 0),
            ],
        )
        .unwrap();
        assert_eq!(plan.selected_run_ids, vec![run(2), run(3), run(4)]);
        assert_eq!(
            plan.bypass_updates,
            vec![BypassUpdate {
                run_id: run(1),
                head_bypass_count: MAXIMUM_HEAD_BYPASS,
            }]
        );
        assert_eq!(plan.stopped_by, Some(DispatchStop::HeadReservation));
    }

    #[test]
    fn a_head_at_the_cap_reserves_resources_without_backfill() {
        let plan = plan_dispatch(
            capacity(),
            ResourceUsage {
                cpu_slots: 4,
                memory_mib: 0,
            },
            false,
            &[queued(1, 8, 4_096, 3), queued(2, 1, 512, 0)],
        )
        .unwrap();
        assert!(plan.selected_run_ids.is_empty());
        assert!(plan.bypass_updates.is_empty());
        assert_eq!(plan.stopped_by, Some(DispatchStop::HeadReservation));
    }

    #[test]
    fn external_pressure_pauses_only_new_dispatch() {
        let plan = plan_dispatch(
            capacity(),
            ResourceUsage {
                cpu_slots: 4,
                memory_mib: 2_048,
            },
            true,
            &[queued(1, 1, 512, 0)],
        )
        .unwrap();
        assert!(plan.selected_run_ids.is_empty());
        assert_eq!(plan.stopped_by, Some(DispatchStop::ExternalMemoryPressure));
    }

    #[test]
    fn impossible_or_corrupt_resource_rows_are_rejected() {
        assert_eq!(
            plan_dispatch(
                capacity(),
                ResourceUsage::default(),
                false,
                &[queued(1, 9, 512, 0)],
            ),
            Err(SchedulerError::RequestExceedsCapacity(run(1)))
        );
        assert_eq!(
            plan_dispatch(
                capacity(),
                ResourceUsage {
                    cpu_slots: 9,
                    memory_mib: 0,
                },
                false,
                &[],
            ),
            Err(SchedulerError::InvalidActiveUsage)
        );
    }
}
