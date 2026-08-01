---
title: Getting started with Trajecta
description: Supported platforms, archive verification, installation, and the path to a first verified Trajecta result.
---

# Getting started

The release package is self-contained for routine command-line use. The CFSR
demonstration data is a separate release asset, so installing a new binary does
not duplicate meteorological files.

## Supported platforms

| Platform | Architecture | Package | Reader support |
|---|---|---|---|
| Windows | x86_64 | ZIP | Rust and packaged native reader |
| Ubuntu 24.04 | x86_64 | `tar.gz` | Rust and packaged native reader |

Contributor builds may work elsewhere. Such builds do not carry release
support during the alpha series.

## Install and verify

1. Download the platform archive from the
   [0.1.0-alpha.1 release](https://github.com/origin652/trajecta/releases/tag/v0.1.0-alpha.1).
2. Download the matching `.sha256` file.
3. Compute the archive digest before extraction.

=== "Windows PowerShell"

    ```powershell
    Get-FileHash .\trajecta-0.1.0-alpha.1-windows-x86_64.zip -Algorithm SHA256
    Expand-Archive .\trajecta-0.1.0-alpha.1-windows-x86_64.zip -DestinationPath .
    ```

=== "Ubuntu 24.04"

    ```bash
    sha256sum trajecta-0.1.0-alpha.1-linux-x86_64.tar.gz
    tar -xzf trajecta-0.1.0-alpha.1-linux-x86_64.tar.gz
    ```

Compare all 64 hexadecimal characters with the release checksum. Stop if any
character differs.

## First result

Continue with the [fifteen-minute quickstart](quickstart.md). It exercises the
same configuration, finalization, run, and verification surfaces used by a
larger study.
