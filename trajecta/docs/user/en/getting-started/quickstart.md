---
title: Fifteen-minute domain-fill quickstart
description: Run and fully verify a small domain-filling water-vapor study with the four-frame CFSR demonstration dataset.
---

# Fifteen-minute domain-fill quickstart

This walkthrough runs a complete domain-fill moisture project. Trajecta creates
1,000 air-mass particles over the global CFSR pressure-level grid and advances
them for ten simulated minutes. Particle states are written at the beginning
and end of the interval. The example is small enough to finish quickly while
using the same Case, Profile, DatasetLock, queue, worker, SQLite sink, and result
commands as a larger study.

The four CFSR frames occupy about 22 MiB. With a release package, the target is
fifteen minutes from extraction to a verified result on a supported machine;
download time is excluded. A first source build takes longer because Cargo must
compile the workspace.

## 1. Obtain the executable

You may use the release package from the [installation page](index.md), or
compile the CLI from the repository.

| Build input | Quickstart requirement |
|---|---|
| Rust | 1.85.0 for the commands below; the workspace minimum is 1.85 |
| Host toolchain | 64-bit MSVC build tools on Windows; a C/C++ build toolchain on Ubuntu |
| Python | 3.11 or newer for `fetch_trajecta_data.py` |
| ecCodes | The example selects the Rust reader; packaged native readers use 2.47.0 |
| netCDF-C | The example selects the Rust reader; packaged native readers use 4.9.3 |
| HDF5 | The example selects the Rust reader; packaged native readers use 1.14.6 |

To build the executable:

=== "Windows PowerShell"

    ```powershell
    git clone https://github.com/origin652/trajecta.git
    Set-Location .\trajecta\trajecta
    rustup toolchain install 1.85.0 --profile minimal
    rustup override set 1.85.0
    cargo build --locked --release -p trajecta-cli
    .\target\release\trajecta-cli.exe --help
    ```

=== "Ubuntu 24.04"

    ```bash
    git clone https://github.com/origin652/trajecta.git
    cd trajecta/trajecta
    rustup toolchain install 1.85.0 --profile minimal
    rustup override set 1.85.0
    cargo build --locked --release -p trajecta-cli
    ./target/release/trajecta-cli --help
    ```

The source-build binary is
`target/release/trajecta-cli.exe` on Windows and
`target/release/trajecta-cli` on Ubuntu. A release archive contains
`trajecta.exe` or `trajecta` at its root. Run the remaining commands from the
directory that contains `examples/` and `tools/`.

The command blocks below use the short name `trajecta`. Define it once for the
current shell:

=== "Windows release package"

    ```powershell
    $TrajectaBinary = (Resolve-Path .\trajecta.exe).Path
    function trajecta { & $TrajectaBinary @args }
    ```

=== "Windows source build"

    ```powershell
    $TrajectaBinary = (Resolve-Path .\target\release\trajecta-cli.exe).Path
    $env:TRAJECTA_BIN = $TrajectaBinary
    function trajecta { & $TrajectaBinary @args }
    ```

=== "Ubuntu release package"

    ```bash
    TRAJECTA_BINARY="$(pwd)/trajecta"
    trajecta() { "$TRAJECTA_BINARY" "$@"; }
    ```

=== "Ubuntu source build"

    ```bash
    TRAJECTA_BINARY="$(pwd)/target/release/trajecta-cli"
    export TRAJECTA_BIN="$TRAJECTA_BINARY"
    trajecta() { "$TRAJECTA_BINARY" "$@"; }
    ```

Confirm the command before continuing:

```text
trajecta --help
```

## 2. Download and verify the data

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

The extracted data directory contains four files at 00, 06, 12, and 18 UTC on
1 January 2009. The ten-minute run uses only part of that day, while the wider
file set supplies the temporal frames expected by the dataset profile. The
individual sizes and hashes are listed on the [demonstration data](demo-data.md)
page.

## 3. Read the example project

The example is stored under `examples/domain-fill-cfsr`. Its project index
names one Case, `moisture`, and one RunProfile, `product`.

| Setting | Value used here |
|---|---|
| Physical interval | 2009-01-01 06:00:00 to 06:10:00 UTC |
| Direction | Forward |
| Population | `domain_fill_air_mass`, 1,000 particles |
| Meteorology | Logical dataset `cfsr`, global periodic domain |
| Integrator | `rk2_spherical/v0`, 300-second step |
| Boundaries | Surface reflection, model-top termination, periodic longitude |
| Output | Particle state at both endpoints, written to SQLite |
| Random seed | 4202 |

Open `cases/moisture.yaml` when you want to change the simulation. Local paths
and resource requests belong to `profiles/product.yaml`. The project index maps
the logical dataset name `cfsr` to profile `cfsr-pgbl-pressure-v0`.

## 4. Create a local machine configuration

All commands below run from the extracted package root.

```text
trajecta --config quickstart.toml config init
trajecta --config quickstart.toml config set resources.cpu_slots 1
trajecta --config quickstart.toml config set resources.memory_reserve_mib 256
trajecta --config quickstart.toml config set resources.memory_pool_mib 1536
trajecta --config quickstart.toml config validate
```

`cpu_slots` controls how much work the local scheduler may admit.
`memory_pool_mib` is the pool available to Trajecta workers on this machine;
`memory_reserve_mib` leaves space for the daemon, operating system, and other
processes. The example Profile asks for one worker thread and 1 GiB, so the
values above admit one run.

The configuration is machine-local. It is kept outside the example project and
can be reused by other projects on the same computer.

## 5. Validate the project and inspect the data plan

```text
trajecta --project examples/domain-fill-cfsr project validate
trajecta --format json --project examples/domain-fill-cfsr project data-plan --output data-plan.json
python tools/fetch_trajecta_data.py --project examples/domain-fill-cfsr --plan data-plan.json
```

The helper previews the request and target paths by default. At this point it
reports the four required CFSR frames as present. Finalization in the next step
creates the DatasetLock.

Before finalization, a missing lock is expected. In JSON output, the data plan
contains one requirement with these main values:

```json
{
  "case_name": "moisture",
  "profile_name": "product",
  "dataset_id": "cfsr",
  "dataset_profile": "cfsr-pgbl-pressure-v0",
  "reader_backend": "rust",
  "required_capabilities": [
    "domain_fill",
    "near_surface_transport",
    "transport"
  ],
  "status": "partial"
}
```

`partial` means the project documents resolve and the local lock has not yet
been created. The data helper reads the plan, inspects the files under the
declared data root, and reports each file as `present` when its content matches.

## 6. Finalize and inspect the environment

```text
trajecta --config quickstart.toml --project examples/domain-fill-cfsr project finalize
trajecta --config quickstart.toml --project examples/domain-fill-cfsr doctor --deep
```

`project finalize` verifies local input content and writes the lock explicitly.
The deep doctor checks the selected configuration, data, lock, filesystem, and
a real SQLite WAL/checkpoint cycle.

The new file `examples/domain-fill-cfsr/locks/cfsr.lock.json` records the four
file hashes, valid times, grid signature, pressure-level signature, dataset
profile, and capabilities. Running `project finalize` again with unchanged
inputs reuses the existing lock. A changed Case or changed data file requires a
new data plan and finalization.

## 7. Run in the foreground

```text
trajecta --config quickstart.toml --project examples/domain-fill-cfsr run --profile product
```

Foreground wait is the default. Keep the reported `job_series_id`, `run_id`,
and result path. After admission, the local daemon schedules the job and the
worker performs the numerical run and writes its result.

A successful machine envelope contains `state: "complete"`, `attempt: 1`, the
allocated resources, and `output_directory`. The result path has this shape:

```text
examples/domain-fill-cfsr/runs/domain-fill-cfsr/RUN_ID/
```

For a later study, add `--detach` when you want the command to return after
admission. Use `job events JOB_ID --follow` or `job wait JOB_ID` to follow that
run.

## 8. Verify and read the result

Replace `RESULT` with the reported run directory or job-series identity.

```text
trajecta --config quickstart.toml result verify RESULT --full
trajecta --config quickstart.toml result inspect RESULT
trajecta --config quickstart.toml result trajectory RESULT --particle-id 0
trajecta --config quickstart.toml run report --result RESULT
```

A successful full verification reports `complete`. `result inspect` shows
1,000 particles, two output events, 2,000 particle-state rows, and zero abnormal
terminations. The exact run identifier and file hashes differ between new
attempts.

The result directory contains these files:

| File | Use |
|---|---|
| `run-manifest.json` | Run status, inputs, numerics, counts, quality, and file identities |
| `particles.sqlite` | Particle, state, mass, event, and termination tables |
| `particles.sqlite-wal` | Empty at successful terminal completion |
| `resolved-case.json` | The Case executed by the worker |
| `resolved-run-profile.json` | The local binding used for this run |
| `provenance-bundle.json` | Meteorological records and transformations used by output records |
| `run-report.md` | Readable summary generated by `run report` |

`result trajectory` emits both endpoint records for particle 0. Compare their
timestamps, coordinates, pressure, height, and sampled field quality to see how
one particle moved during the ten-minute interval. For larger selections, use
JSONL output and redirect it to an analysis file.

## Completion check

- The manifest status is `complete`.
- The abnormal particle count is zero.
- Full verification succeeds.
- `particles.sqlite` passes the recorded integrity and row-count checks.
- `provenance-bundle.json` matches the manifest identity.

If any item fails, preserve the result directory and use the
[troubleshooting index](../operations/troubleshooting.md).

You have now completed the same project lifecycle used by the longer
tutorials. Continue with the [domain-fill moisture tutorial](../tutorials/domain-fill.md)
to learn how the air-mass population is constructed and how to extend the
simulation period, particle count, and output schedule.
