---
title: Release policy
description: How Trajecta versions software, schemas, data assets, documentation snapshots, and alpha compatibility.
---

# Release policy

Trajecta release notes connect one software version to its packages, public
formats, documentation, and known limits. The current channel is alpha, so each
release page also identifies the parts intended for automation and the parts
still open to change.

## Version identifiers

Several identifiers appear in a working project. They advance for different
reasons:

| Identifier | Example | Changes when |
| --- | --- | --- |
| Software version | `0.1.0-alpha.1` | A new executable release is prepared |
| Git tag | `v0.1.0-alpha.1` | The source commit for that software release is selected |
| Document schema | `trajecta.config/v1` | The corresponding disk or stream format needs a new contract |
| SQLite user version | `1` | The result database schema changes incompatibly |
| DatasetLock schema | Version inside the lock | Dataset identity or lock semantics change |
| Demonstration-data asset | `trajecta-demo-cfsr-20090101-v1` | The sample file set, metadata, or packaging contract changes |
| Documentation version | `0.1.0-alpha.1` or `dev` | A software snapshot is published, or accepted docs move on |

A software release does not automatically increment every schema. Automation
should read the format's own `schema_version` before parsing command-specific
fields.

## Release channels

| Channel | URL behavior | Content |
| --- | --- | --- |
| Named release | Immutable version path; current default is `0.1.0-alpha.1` | Manual matching a published software version |
| `latest` alias | Resolves to the current named release | Convenient stable documentation entry |
| `dev` | Mutable `/dev/` path with search-engine `noindex` | Documentation for accepted changes after the latest snapshot |

Ordinary documentation corrections update `dev`. A release event creates the
named snapshot after source, package, and documentation gates pass for the same
version.

## Contents of a release

A complete release entry provides:

- Windows x86_64 and Ubuntu 24.04 x86_64 product archives;
- a SHA-256 file beside each archive;
- a separately downloadable demonstration dataset and checksum;
- release notes with workflow highlights and known limits;
- the bilingual versioned manual;
- package build manifests, software bills of materials, and license inventories
  inside each archive;
- validation data and charts through the manual's Validation section.

The archive names, native component versions, and supported reader matrix are
listed in [Platform and reader matrix](../reference/platforms.md).

## Compatibility in the alpha series

The CLI, documented configuration, public schemas, machine envelopes, and result
artifacts form the product boundary. A change to one of these surfaces is
accompanied by updated examples, validation, and release notes.

Alpha releases may revise defaults, fields, or behavior before a stable 1.0
contract is established. A project intended for repeated or audited work should
retain:

- the product archive or its exact checksum;
- resolved Case and RunProfile documents;
- DatasetLocks and source file hashes;
- the terminal run manifest and provenance bundle;
- the release documentation snapshot used for the run.

Public Rust modules are contributor interfaces during alpha. Their source-level
compatibility is described in [Rust API](../developer/api.md).

## Documentation corrections

The immutable snapshot preserves the manual that accompanied a release. Minor
clarifications first appear in `dev`. If a correction affects a command, format,
scientific interpretation, or safety procedure, the release note records the
scope and links to the corrected dev page until a later snapshot is published.

Raw validation files and package checksums retain their original identities.
Charts are regenerated from the same frozen inputs when only presentation is
adjusted.

## Reading a release page

Each named page follows the same order:

1. release status and supported packages;
2. scientific workflows and data families;
3. runtime and result capabilities;
4. documentation and demonstration data;
5. compatibility notes and known limits;
6. links to installation, quickstart, and exact reference material.

The first published entry is [0.1.0-alpha.1](0.1.0-alpha.1.md).
