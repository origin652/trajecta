---
title: ERA5 hybrid-level ozone tutorial
description: Run a backward stratospheric-ozone domain-fill workflow with prepared ERA5 hybrid-level data.
---

# ERA5 hybrid-level ozone workflow

The ozone population begins from a domain-filled air-mass population and
applies the frozen PV60 stratospheric-ozone rule. This tutorial runs backward
with prepared 137-level ERA5 hybrid meteorology.

## Project binding

--8<-- "examples/ozone-era5-hybrid/trajecta-project.yaml"

--8<-- "examples/ozone-era5-hybrid/profiles/product.yaml"

The complete Case is stored at `examples/ozone-era5-hybrid/cases/ozone.yaml`.
Its rule identifier, seed, boundaries, direction, and particle count are part
of the resolved Case identity.

## Prepare and execute

```text
trajecta --project examples/ozone-era5-hybrid project data-plan --output hybrid-plan.json
python tools/fetch_trajecta_data.py --project examples/ozone-era5-hybrid --plan hybrid-plan.json
python tools/fetch_trajecta_data.py --project examples/ozone-era5-hybrid --plan hybrid-plan.json --execute
trajecta --project examples/ozone-era5-hybrid project finalize
trajecta --project examples/ozone-era5-hybrid run --profile product
```

ERA5 hybrid input requires 3-D model-level fields, logarithmic surface
pressure, surface companions, and preparation metadata. Finalization must
reject incomplete capability coverage. For a backward case, interpret time in
physical order while retaining the recorded execution direction.

Use full verification before comparing ozone mass or particle state across
runs. Preserve the exact data lock and prepared-file identities.
