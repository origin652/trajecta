---
title: Particle population models
description: Compare release-driven, dry-air domain-fill, and stratospheric-ozone populations in Trajecta.
---

# Particle population models

| Model | Birth definition | Carried state | Primary use |
|---|---|---|---|
| Release-driven | Scheduled events with geometry and mass | One or more substances | Sources and receptor trajectories |
| Dry-air domain fill | Stratified sampling of a meteorological domain | Dry-air mass and humidity context | Moisture and air-mass tracking |
| Stratospheric ozone domain fill | Air-mass fill plus frozen ozone rule | Dry-air and ozone mass | Ozone transport studies |

All models share the same trajectory integrator, meteorology query boundary,
lifecycle accounting, output sink, and provenance system. Their initialization
and carried mass differ.

A Case chooses exactly one population strategy. Population identifiers,
release event identifiers, substance identifiers, domain references, seed, and
target count become part of resolved scientific identity. Changing any of them
requires a new run and, when capabilities or coverage change, a new data plan
and DatasetLock.
