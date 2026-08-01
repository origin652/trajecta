---
title: Scheduled release tutorial
description: Run a point-source release experiment with explicit mass, geometry, height, and reproducible output.
---

# Scheduled release tutorial

A release-driven population creates particles from one or more events. Each
event defines a time interval, particle count, substance mass, geometry, and
vertical placement. This model is suitable when a source is part of the
scientific question.

## Case source

--8<-- "examples/release-cfsr/cases/release.yaml"

The example releases 1,000 particles at one point and assigns one kilogram of
the `water` tracer across the event. Adjust the mass and geometry to match the
study; do not interpret the tutorial value as an emission inventory.

## Execute and read

Copy or link the four demonstration frames into `examples/release-cfsr/data`,
then run:

```text
trajecta --project examples/release-cfsr project validate
trajecta --project examples/release-cfsr project finalize
trajecta --project examples/release-cfsr run --profile product
trajecta result verify RESULT --full
```

Use `result trajectory` for individual paths. Use `result inspect` for release,
termination, lifecycle, and quality totals. Save the resolved Case and Profile
from the result directory with any publication analysis.
