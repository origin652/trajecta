---
title: FLEXPART comparability matrix
description: Exact, aligned, aggregate-only, and non-comparable dimensions in the frozen Trajecta and FLEXPART validation.
---

# FLEXPART comparability matrix

Comparability is defined per field and measurement. A shared label such as
"trajectory" does not establish equal numerics or equal output products.

| Dimension | Status | Interpretation |
|---|---|---|
| Input source fields | Exact | Same frozen meteorological source content |
| Encoded meteorological values | Aligned | Common values, units, and physical interpretation audited |
| Output instants | Exact | Three common physical times at 20, 40, and 60 minutes |
| Particle count and released mass | Exact | Equal contract values |
| Longitude, latitude, height ASL | Aligned | Common coordinate meaning for ensemble metrics |
| Ensemble measures | Aggregate-only | Centroid, dispersion, transport, and occupancy compared |
| Particle trajectory crosswalk | Not comparable | Different sampling RNG and integrator prevent ID pairing |
| SQLite/provenance overhead | Not comparable | No equivalent FLEXPART audit product in this matrix |

The matrix supports method-specific claims. It does not establish equivalence
for chemistry, deposition, turbulence, long-duration transport, other grids,
or production-scale output schedules.

The scientific chart uses the recorded 20, 40, and 60 minute instants. The
original frozen chart is retained with the aggregate under raw evidence; the
publication copy corrects its x-axis labels without changing any data value.
