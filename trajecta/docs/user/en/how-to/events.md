---
title: Follow Trajecta job events
description: Read durable queue events with a global cursor, filter one job series, reconnect safely, and consume JSONL streams.
---

# Follow job events

Trajecta stores state transitions, progress, resource samples, warnings, errors,
and artifact notifications as job events. Event readers share a persistent log:
reading an event does not remove it or hide it from another reader. A terminal,
a monitoring process, and an automation client can therefore follow the same
queue independently.

## Event identity and ordering

Every event has a global `sequence` number. The first sequence is `1`, and the
number increases across the complete catalog rather than restarting for each
job. `--since N` returns events whose global sequence is strictly greater than
`N`.

Each event also carries:

| Field | Purpose |
| --- | --- |
| `job_series_id` | Selects the logical task across attempts |
| `run_id` | Selects the exact attempt that emitted the event |
| `attempt` | Gives the one-based attempt number |
| `emitted_at` | Records the UTC persistence time |
| `kind` | Classifies the event |
| `state` | Gives the durable state observed after the event |

Progress, resource, diagnostic, and artifact fields appear only when relevant
to the event kind.

## Read a one-shot snapshot

Read all currently available events:

```text
trajecta job events --since 0
```

Filter one logical job series by placing its ID after `events`:

```text
trajecta job events JOB_ID --since 0
```

One request reads at most 10,000 events. Use the last returned global sequence
as the next cursor when a busy catalog contains more rows.

JSON mode returns the current batch in one CLI envelope:

```text
trajecta --format json job events JOB_ID --since 120
```

This form is convenient for a short script that reads, processes, stores its
cursor, and exits.

## Follow one running job

Use `--follow` to keep polling:

```text
trajecta --format jsonl job events JOB_ID --since 0 --follow
```

The stream ends after that job's current attempt reaches a terminal state and
all persisted events through that point have been emitted. Its final JSONL
record has `kind: "summary"`; `summary.run_success` is true only when the job
state is `complete`.

Human output can also be followed:

```text
trajecta job events JOB_ID --follow
```

Each line begins with the durable event sequence, followed by the observed
state, event kind, and optional message.

## Follow the complete queue

Omit the job ID to receive events from every series:

```text
trajecta --format jsonl job events --since SEQUENCE --follow
```

This stream has no single job terminal condition. It stays open and continues
to poll until the client is stopped or a read error occurs. It is suitable for
a local dashboard, log adapter, or queue-status collector.

Because the cursor is global, one stored number is sufficient for the complete
queue. A per-job consumer still stores the global sequence from the last event
it processed.

## Consume JSONL records

Every line is an independent `trajecta.cli-stream-item/v1` JSON object. The
`kind` field determines the payload:

| `kind` | Payload |
| --- | --- |
| `data` | A `trajecta.job-event/v1` record in `data` |
| `diagnostic` | A structured read error in `diagnostic` |
| `summary` | Stream completion status in `summary` |

The outer stream-item `sequence` counts lines in the current CLI stream. The
event's `data.sequence` is the durable global cursor. Reconnection uses the
latter.

A compact processing loop follows this pattern:

1. Read one line and parse the stream item.
2. For `kind: "data"`, process `data` and persist `data.sequence` after the
   side effect succeeds.
3. For `kind: "diagnostic"`, record the diagnostic code and message.
4. For `kind: "summary"`, close the stream and read `summary.ok` and, when
   present, `summary.run_success`.

Storing the cursor after the consumer's own work succeeds gives at-least-once
delivery across a client crash. Storing it first gives at-most-once delivery.
Choose the order according to the downstream operation.

## Reconnect after a client interruption

Suppose the last completely processed event had global sequence `4281`:

```text
trajecta --format jsonl job events JOB_ID --since 4281 --follow
```

The next batch begins with the first matching event above that number. Reusing
an earlier cursor repeats durable records; using a later cursor skips them.
Trajecta does not maintain a cursor on behalf of each reader.

!!! tip "Save the cursor after processing"

    This order may repeat one event after a crash, but it will not skip an event
    whose downstream work never finished.

## Interpret event kinds

| Kind | Typical content |
| --- | --- |
| `state_transition` | Queue admission, worker startup, running, cancellation, or terminal state |
| `progress` | Completed macro steps, current simulation time, active particles, and termination counts |
| `resource` | Wall time plus available CPU-time and RSS observations |
| `warning` | A recoverable condition such as external memory pressure |
| `error` | A fatal or terminal diagnostic code and message |
| `artifact` | Creation or finalization of an auditable output path |

Progress and resource events are sampled at the cadence configured by
`monitoring.sample_interval_ms`, subject to bounded runtime emission. They are
suited to status displays and operational history; result data remains in the
run directory.

## Read the current snapshot alongside events

An event describes a transition at a point in the persistent history. Query
`job status` when the consumer needs the latest queue position, result path, or
terminal timestamp:

```text
trajecta --format json job status JOB_ID
```

After a terminal event, use the reported output directory or the job ID with
the [result commands](results.md).
