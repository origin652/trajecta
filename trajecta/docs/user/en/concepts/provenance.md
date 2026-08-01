---
title: Provenance and result directories
description: Learn how manifests, SQLite output, resolved inputs, provenance bundles, reports, and forensic artifacts relate.
---

# Provenance and result directories

A result directory is an immutable evidence boundary after terminalization.
Its main artifacts are:

| Artifact | Purpose |
|---|---|
| `run-manifest.json` | Lifecycle, identities, counts, quality, input inventory, output references |
| `particles.sqlite` | Particle state and output-event records |
| `provenance-bundle.json` | Content-addressed input, configuration, executable, and output identity |
| `resolved-case.json` | Scientific intent executed by the worker |
| `resolved-run-profile.json` | Local execution binding admitted for the run |
| `run-report.md` | Deterministic human-readable derived view |
| Forensic entries | Preserved interruption or force-cancel evidence when present |

The manifest points to SQLite and provenance identities. Full verification
recomputes content and semantic digests rather than relying on file existence.
The report is excluded from scientific digests, so regenerating it remains
idempotent.

Copy a result directory only after the worker reaches a terminal state and WAL
handling is complete. Preserve the directory as a unit. For routine analysis,
use `result inspect`, `result verify`, and `result trajectory` before opening
SQLite directly.
