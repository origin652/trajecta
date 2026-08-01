---
title: ERA5 pressure-level air-mass tutorial
description: Prepare and run a limited-domain air-mass population with ERA5 pressure-level meteorology.
---

# ERA5 pressure-level air-mass workflow

This project applies dry-air domain filling to a limited ERA5 pressure-level
domain. The Case selects the logical dataset and domain. The RunProfile binds
that dataset to local files and the `era5-cf-pressure-netcdf-v0` profile.

## Project sources

--8<-- "examples/air-mass-era5-pressure/trajecta-project.yaml"

--8<-- "examples/air-mass-era5-pressure/cases/air-mass.yaml"

## Prepare data before finalization

```text
trajecta --project examples/air-mass-era5-pressure project data-plan --output era5-plan.json
python tools/fetch_trajecta_data.py --project examples/air-mass-era5-pressure --plan era5-plan.json
```

Review the provider request, target paths, times, variables, pressure levels,
and area. Supply CDS credentials through the official CDS configuration or
environment. Credentials must not enter the project, logs, or provenance.
Add `--execute` only after approving the request.

```text
python tools/fetch_trajecta_data.py --project examples/air-mass-era5-pressure --plan era5-plan.json --execute
trajecta --project examples/air-mass-era5-pressure project finalize
trajecta --project examples/air-mass-era5-pressure run --profile product
```

Limited-domain outflow is a normal termination. Investigate invalid
meteorology, reflection limits, and lifecycle mismatches as abnormal outcomes.
