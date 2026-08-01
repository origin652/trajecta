---
title: Release policy
description: Software and documentation version policy for Trajecta prereleases and immutable documentation snapshots.
---

# Release policy

Software versions follow Cargo prerelease semantics. A release tag freezes the
source, product packages, checksums, demonstration data, validation evidence,
and bilingual documentation snapshot.

Documentation uses `mike`. The `dev` version follows accepted changes and is
excluded from search-engine indexing. Release snapshots are immutable. The
default release alias currently resolves to `0.1.0-alpha.1`.

A documentation correction does not create a software version. If a published
release page needs a material correction, record the correction transparently
and preserve the original evidence identity.
