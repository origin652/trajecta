---
title: Trajecta validation evidence policy
description: Scope and acceptance rules for scientific, performance, determinism, product, and platform validation.
---

# Validation evidence policy

Validation records a contract, frozen identities, execution conditions, raw
measurements, derived summaries, and acceptance gates. A result is reported as
passed only when every hard gate in that contract succeeds.

## Evidence layers

| Layer | Question |
|---|---|
| Input identity | Were the intended source bytes and dataset profile used? |
| Meteorology | Do readers expose aligned physical fields and query coverage? |
| Numerical execution | Are lifecycle, mass, finite values, boundaries, and output order valid? |
| Determinism | Do repeated and cross-worker runs preserve specified digests? |
| Product | Are manifest, SQLite, WAL, provenance, report, and CLI behavior valid? |
| Performance | What is measured under a declared host and workload contract? |
| Platform | Does a clean release package pass on each supported system? |

Scientific evidence and product evidence are retained together. Timing alone
cannot make a failed scientific run acceptable. A scientific comparison does
not imply interchangeable configuration, implementation, or output products.

The current cross-model evidence is documented only in this Validation area.
Routine user pages describe Trajecta on its own product contracts.
