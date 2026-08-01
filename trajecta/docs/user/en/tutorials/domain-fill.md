---
title: Domain-fill moisture tutorial
description: Configure, run, and interpret a reproducible CFSR domain-filling water-vapor experiment.
---

# Domain-fill moisture tutorial

Domain filling maintains a population that represents air mass over a selected
meteorological domain. The model assigns dry-air mass at initialization and
accounts for later boundary births or normal terminations through the mass
ledger. Specific humidity sampled from meteorology supplies the water-vapor
state used for moisture analysis.

## Case source

--8<-- "examples/domain-fill-cfsr/cases/moisture.yaml"

The fixed random seed makes population sampling reproducible for the same
resolved Case, RunProfile, data locks, reader, and executable identity.

## Run the project

Prepare the quickstart dataset, then run:

```text
trajecta --project examples/domain-fill-cfsr project data-plan --output data-plan.json
trajecta --project examples/domain-fill-cfsr project finalize
trajecta --project examples/domain-fill-cfsr run --profile product
```

Inspect the result at two levels:

```text
trajecta result inspect RESULT
trajecta result trajectory RESULT --particle-id 0
```

The summary population count describes lifecycle coverage. The trajectory
stream describes one particle through ordered output events. Scientific use
should also inspect the mass ledger, normal termination reasons, finite-value
audit, and full verification result.

## Extend the study

Increase the end time first, regenerate the data plan, and provide complete
meteorological anchors on both sides of every query time. Increase
`target_particle_count` after measuring memory use with the intended output
schedule. Preserve the previous result directory as an independent attempt.
