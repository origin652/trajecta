---
title: Fifteen-minute domain-fill quickstart
description: Run and fully verify a small domain-filling water-vapor study with the four-frame CFSR demonstration dataset.
---

# Fifteen-minute domain-fill quickstart

This walkthrough runs 1,000 air-mass particles for ten simulated minutes over
the global CFSR grid. The four source frames occupy about 22 MiB. The target is
fifteen minutes from an extracted package to a fully verified result on a
supported machine; download time is excluded.

## 1. Download and verify the data

Download
[`trajecta-demo-cfsr-20090101-v1.zip`](https://github.com/origin652/trajecta/releases/download/v0.1.0-alpha.1/trajecta-demo-cfsr-20090101-v1.zip).
Its frozen archive SHA-256 is:

```text
cd2c38083130c014daac4b4fcde0680d3f3d6bc2df43daf22c8cd015e95919f8
```

Verify the archive, extract it, and copy the four files into the example data
directory.

=== "Windows PowerShell"

    ```powershell
    Get-FileHash .\trajecta-demo-cfsr-20090101-v1.zip -Algorithm SHA256
    Expand-Archive .\trajecta-demo-cfsr-20090101-v1.zip -DestinationPath .\demo-data
    Copy-Item .\demo-data\trajecta-demo-cfsr-20090101-v1\data\* `
      .\examples\domain-fill-cfsr\data\
    ```

=== "Ubuntu 24.04"

    ```bash
    sha256sum trajecta-demo-cfsr-20090101-v1.zip
    unzip trajecta-demo-cfsr-20090101-v1.zip -d demo-data
    cp demo-data/trajecta-demo-cfsr-20090101-v1/data/* \
      examples/domain-fill-cfsr/data/
    ```

The individual file identities are listed in [Demonstration data](demo-data.md).

## 2. Select a local configuration

All commands below run from the extracted package root.

```text
trajecta --config quickstart.toml config init
trajecta --config quickstart.toml config set resources.cpu_slots 1
trajecta --config quickstart.toml config set resources.memory_reserve_mib 256
trajecta --config quickstart.toml config set resources.memory_pool_mib 1536
trajecta --config quickstart.toml config validate
```

## 3. Validate the project and data plan

```text
trajecta --project examples/domain-fill-cfsr project validate
trajecta --format json --project examples/domain-fill-cfsr project data-plan --output data-plan.json
python tools/fetch_trajecta_data.py --project examples/domain-fill-cfsr --plan data-plan.json
```

The helper is dry-run by default. At this point it should report that the four
required CFSR frames already exist. It does not create a DatasetLock.

## 4. Finalize and inspect the environment

```text
trajecta --config quickstart.toml --project examples/domain-fill-cfsr project finalize
trajecta --config quickstart.toml --project examples/domain-fill-cfsr doctor --deep
```

`project finalize` verifies local input content and writes the lock explicitly.
The deep doctor checks the selected configuration, data, lock, filesystem, and
a real SQLite WAL/checkpoint cycle.

## 5. Run in the foreground

```text
trajecta --config quickstart.toml --project examples/domain-fill-cfsr run --profile product
```

Foreground wait is the default. Keep the reported `job_series_id`, `run_id`,
and result path. Closing the client does not erase an admitted job; the local
daemon and worker own execution after admission.

## 6. Verify and read the result

Replace `RESULT` with the reported run directory or job-series identity.

```text
trajecta --config quickstart.toml result verify RESULT --full
trajecta --config quickstart.toml result inspect RESULT
trajecta --config quickstart.toml result trajectory RESULT --particle-id 0
trajecta --config quickstart.toml run report --result RESULT
```

A successful full verification reports `complete` and matching canonical,
SQLite, and provenance digests. `run-report.md` is a derived view; creating it
does not change scientific identity.

## Completion check

- The manifest status is `complete`.
- The abnormal particle count is zero.
- Full verification succeeds.
- `particles.sqlite` passes the recorded integrity and row-count checks.
- `provenance-bundle.json` matches the manifest identity.

If any item fails, preserve the result directory and use the
[troubleshooting index](../operations/troubleshooting.md).
