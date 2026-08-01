---
title: Packaging and release
description: Deterministic product archives, native runtime inventory, release evidence, version policy, and GitHub publication flow.
---

# Packaging and release

Formal product archives are built on Windows x86_64 and Ubuntu 24.04 x86_64.
Each archive contains the binary, native libraries, ecCodes definitions,
examples, offline recovery docs, the data helper, license material, an SBOM, and
`BUILD-MANIFEST.json`.

The package builder uses a fixed source identity and source date. Archive paths,
permissions, timestamps, payload ordering, and hashes are deterministic. Package
verification checks the adjacent SHA-256, manifest shape, every payload digest,
the binary identity, supply-chain files, and native component versions.

## Release sequence

1. Pass workspace, contract, documentation, and product-package gates.
2. Build and verify both formal archives in clean extraction directories.
3. Run the domain-fill quickstart and product smoke on both platforms.
4. Build the demonstration-data asset and verify its frozen manifest.
5. Create the immutable documentation snapshot with `mike`.
6. Publish software archives, checksums, data asset, and release notes.
7. Check the public Pages release URL, sitemap, canonical metadata, and language switch.

M5.1 retains software version `0.1.0-alpha.1`. Documentation commits update the
`dev` site and do not create another release version. Release publication must
not proceed while provider terms or GitHub authentication remain unresolved.
