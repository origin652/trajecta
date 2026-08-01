---
title: Domain-fill water-vapor tracking
description: Scientific scope, mass representation, boundary exchange, and interpretation of Trajecta domain filling.
---

# Domain-fill water-vapor tracking

Domain filling represents an atmospheric domain with a finite particle
population. Each particle carries a dry-air mass derived from the represented
cell or stratum. Meteorological humidity relates that dry-air representation to
water-vapor content along the trajectory.

## Initialization

The selected domain and target count define a stratified population. The
frozen random dimensions choose mass stratum, longitude, latitude, and pressure
within that representation. A recorded seed makes those choices reproducible.

## Boundary exchange

During execution, flow through an open boundary can create inflow particles or
terminate outflow particles. These are normal lifecycle events when the Case
selects a limited-domain boundary policy. The mass ledger records initial mass,
birth mass, termination mass, and final active mass at each output step.

## Interpretation

Particle count alone is not conserved when boundary exchange occurs. Evaluate
mass closure, population reasons, humidity state, and spatial sampling together.
An abnormal termination indicates a numerical or input-quality condition and
must remain separate from normal physical outflow.

Domain filling provides a sampling representation. Resolution, particle count,
meteorological cadence, boundary policy, and output schedule all affect the
scientific question that can be resolved.
