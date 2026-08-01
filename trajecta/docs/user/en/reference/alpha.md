---
title: Alpha limits
description: Explicit scope and compatibility limits of Trajecta 0.1.0-alpha.1.
---

# Alpha limits

`0.1.0-alpha.1` is suitable for evaluated research workflows with retained
inputs and verification evidence. It is a prerelease and carries these limits:

- Only Windows x86_64 and Ubuntu 24.04 x86_64 product packages are formally
  validated.
- The local daemon exposes local IPC only. Remote scheduling is unavailable.
- `job prune` produces a dry-run plan and deletes nothing.
- Plugin loading and a plugin API have not been implemented.
- No general export command exists. SQLite and the product result commands are
  the current result interfaces.
- Rust crate APIs are contributor-internal and do not have product compatibility
  guarantees.
- Configuration and disk schemas are versioned. Alpha releases may introduce a
  migration requirement when those schema identifiers change.
- A job remains bound to its finalized Case, Profile, DatasetLock, input hashes,
  and attempt identity. Input changes require a new finalization or attempt.
- Provider downloads require network access and user-held credentials where the
  provider requires them. Credentials are outside project and provenance files.

Scientific and performance claims are limited to the frozen evidence described
in [Validation](../validation/index.md). Preserve raw inputs, manifests,
provenance, and result directories when publishing derived findings.
