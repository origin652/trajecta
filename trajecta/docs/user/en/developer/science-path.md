---
title: Meteorology and numerical path
description: Dataset locking, meteorological preparation, interpolation, spherical RK2 integration, boundaries, and domain-fill populations in Trajecta.
---

# Meteorology and numerical path

The scientific path begins with immutable input documents and ends with an
ordered particle batch ready for scheduled output. File interpretation belongs
to `trajecta-met`; particle lifecycle belongs to `trajecta-core`. The boundary
between them is a typed, prepared meteorological query.

This page traces the production path. Module and type names are included so a
contributor can move from the description to the relevant source.

## Input binding

A production run receives a resolved Case, a resolved RunProfile, and one
DatasetLock for each selected dataset profile. Project finalization creates this
binding before queue admission.

The lock carries the parts of the data selection that must remain fixed during
execution:

| Lock content | Runtime use |
| --- | --- |
| Dataset profile and lock schema identity | Select the intended interpretation contract |
| Ordered file inventory and SHA-256 | Detect replacement, truncation, or selection drift |
| Time coverage | Check that start, midpoint, release, boundary, and output queries are bracketed |
| Spatial and vertical signatures | Bind the Case domain to the indexed grid and levels |
| Capability set | Confirm that transport and population fields can be produced |
| Source metadata | Carry dataset identity into the run manifest and provenance |

`trajecta-case` owns the lock shape. The production loader verifies the lock and
uses only declared data roots. It does not discover a more convenient file after
the attempt has started.

## Profile compilation and source inventory

A Dataset Profile describes how source records become canonical fields. Direct
field mappings specify exact GRIB or NetCDF identities, source units, temporal
semantics, and an ordered set of explicitly allowed alternatives. Derived fields
are a typed computation graph.

Profile compilation performs static checks before any particle query:

- graph node identifiers and outputs are unique;
- dependencies form an acyclic graph;
- units and physical dimensions agree across each operation;
- array shape and vertical staggering remain compatible;
- an operation runs only at a legal frame, tile, column, or sample stage;
- required source alternatives and capabilities can be resolved.

The operation set is closed and named. It includes arithmetic and unit
conversion, accumulated-field differencing, thermodynamic functions, hybrid
pressure construction, and native-grid diagnostics. A Profile cannot execute
arbitrary code or perform I/O from a computation node.

The source inventory is built from metadata rather than filenames alone. It
records container format, valid time, grid description, vertical topology, field
identity, and dataset attributes. Exact Profile matchers select records from
that inventory. Duplicate or incompatible candidates become diagnostics instead
of implicit last-file-wins behavior.

## Frame selection

The runner enumerates every physical time that the simulation can query. This
set includes run endpoints, RK2 stage midpoints, continuous-release windows,
population operations, and scheduled output times. The builder then selects a
contiguous set of source frames that brackets the full interval.

An exact query at a source timestamp can still need neighboring frames when a
derived field requires temporal differencing. Warm-up depth is taken from the
dependency closure of the requested capabilities, so accumulated quantities
receive the preceding support required by their graph.

For every selected frame, `trajecta-met` performs the following work:

1. Decode source-native arrays and validity masks.
2. Normalize units while preserving source identity and quality.
3. Execute frame-stage and tile-stage graph nodes.
4. Construct vertical geometry for pressure-level or hybrid-level data.
5. Store canonical fields and provenance assignments in `RawMetFrame`.
6. Place the frame in the bounded `FrameCache` and form a `PreparedWindow` for
   the requested time.

Horizontal topology stays source-native. Preparation does not create a hidden
global regridded dataset. A regular latitude-longitude source is queried through
its own grid backend, with spherical vector handling for wind components.

## Prepare and execute phases

Meteorological queries deliberately have two phases:

```text
MetEngine::prepare_for_domain(time, domain)
    -> PreparedWindow
PreparedWindow::prepare_transport_batch(plan, points, workspace)
    -> PreparedTransportBatch
PreparedTransportBatch::execute(execution_context, workspace)
    -> TransportOutput
```

The prepare phase may select cached frames, construct columns, calculate
horizontal placement, and pin the required stencils. The execute phase receives
only those pinned structures. It has no reader or provider handle and therefore
does not open meteorological files while particles are being sampled.

`BatchWorkspace` owns reusable scratch vectors. Callers can retain it across
batches to avoid repeated allocations. The execution context supplies the
worker count; transport batches below 256 points run serially, while larger
batches use a Rayon pool keyed by that count.

Caches remain part of query preparation and execution mechanics:

| Cache | Key idea | Purpose |
| --- | --- | --- |
| Frame cache | Dataset frame identity | Retain decoded canonical fields within a memory budget |
| Tile and column caches | Field, frame, spatial support, vertical request | Reuse derived neighborhoods and column geometry |
| Boundary stencil session | Exact path-query support | Keep stencils pinned while a boundary path is evaluated |
| Last exact transport query | Window identity, query plan, domain, point arrays and floating-point bits | Reuse only a byte-for-byte equivalent transport request |

The exact transport key includes the frame window, temporal weights, plan,
vertical coordinate, and point values. A changed cohort, coordinate, time, or
domain produces a different key. Cache reuse returns the same typed output and
provenance assignments as executing that exact request.

## Transport sampling

The integrator asks for three transport components at height above sea level:

- eastward wind in metres per second;
- northward wind in metres per second;
- geometric vertical velocity in metres per second.

The query engine locates horizontal support, builds the source-appropriate
vertical column, resolves surface-layer rules, and blends the two temporal
frames. Vector wind interpolation accounts for spherical bases. Scalar fields
and vertical velocity follow their registered interpolation and derivation
semantics.

Each output row contains values plus a `SampleStatus`, field quality, bounds, and
provenance. Status distinguishes conditions such as valid data, below-ground
sampling, surface-layer limits, model-top exit, domain exit, and invalid source
support. The integrator does not infer validity from a sentinel floating-point
value.

Boundary-related status can be handed to the continuous boundary path. For
example, a predictor outside the domain or beyond available vertical support may
still define a segment whose first physical intersection is a normal boundary
termination. A genuinely unavailable transport sample becomes an abnormal
`invalid_meteorology` termination.

## Spherical midpoint RK2

`Rk2Spherical` is the production integrator in `trajecta-core/src/integrator`.
For one particle beginning at time \(t\) with signed step \(\Delta t\), it
performs two batched transport queries:

1. Query velocity at the starting position and time \(t\).
2. Use half of \(\Delta t\) to form a midpoint position.
3. Query velocity at that midpoint and at \(t + \Delta t/2\).
4. Advance the original position through the full step with the midpoint
   velocity.

Horizontal displacement is calculated on a sphere and longitude is normalized.
Vertical motion uses geometric vertical velocity. Forward and backward runs use
the same equations with the sign of \(\Delta t\) changed; particle age increases
by the absolute duration.

Rows born inside a macro step can have distinct exact start times. The timed
integrator groups equal query times in stable order, advances each row once to
the common macro-step endpoint, and reverses time-group traversal for backward
integration. A batch cannot mix forward and backward signs.

Non-finite coordinates, overflow, or a failed spherical displacement terminate
the affected particle as a numerical failure. Other live rows remain in the
batch so the terminal manifest can report the complete population outcome.

## Boundary order

The integrator returns a proposed batch before boundary policies are applied.
The runner then evaluates the continuous path from the previous state to the
proposal. This separation keeps lifecycle order independent of the integrator
implementation.

Production policies cover:

| Boundary | Outcome |
| --- | --- |
| Surface | Reflect the path according to the configured surface policy, subject to its reflection limit |
| Model top | Terminate at the first top intersection |
| Limited horizontal domain | Terminate at the first domain intersection |
| Global longitude | Apply periodic longitude handling |

When several intersections are possible, ordered path segments and intersection
fractions determine the first event. Termination time and position are written
at the intersection, which keeps lifecycle samples monotonic in both integration
directions.

## Population lifecycle

The `PopulationModel` interface owns work around advection:

```text
initialize
pre-step and emission
advection by the shared integrator
post-advection accounting
boundary maintenance
finalize
```

### Release population

A release population creates rows from configured release events. The vertical
resolver queries meteorology when a release coordinate needs pressure, terrain,
or model-level interpretation. Birth time, origin event, mass, and stable
particle identity are set before the row joins an integration cohort.

### Domain-fill air mass

Domain filling derives finite atmospheric cells from the selected native grid
and vertical geometry. Valid layer dry-air mass is calculated from the
meteorological state, then apportioned among particles according to the Case.
The initial cohort records `domain_initial` origin.

For a limited domain, boundary-face layers carry dry-air flux. The integration
direction determines which signed flux is inflow. Mass below one particle share
is retained as a residual and carried into later steps; it is not rounded away
at every boundary event. New rows receive `domain_boundary` origin, face
identity, exact birth time, and stable IDs.

### Stratospheric ozone

The ozone population uses the same domain-fill transport and dry-air accounting
with an ozone-specific seeding rule. Its meteorological capability set includes
the fields required to classify the stratospheric source and assign ozone mass.
Population-specific science stays in the seeding and accounting layer; particle
motion still uses the shared transport query and RK2 implementation.

## Determinism and provenance

Stable ordering is established before parallel query execution. Particle IDs,
birth identities, output event sequences, and sorted SQLite insertion do not
depend on Rayon completion order. Changing worker count can alter wall time but
must preserve the normalized scientific content for the same resolved inputs.

Every sampled or derived meteorological value carries a provenance assignment.
The final bundle records source records and transformations separately from the
particle table, then the manifest binds the bundle and SQLite identities. See
[Output and runtime](runtime.md) for terminalization details.

## Reviewing a scientific-path change

Choose checks according to the rule being changed:

| Change | Minimum focused coverage |
| --- | --- |
| Profile mapping or unit conversion | Profile compile tests and a real source inventory |
| Reader metadata or array layout | Native/pure differential test across all selected times and levels |
| Derived field | Graph type checks, quality/provenance assertions, reference values |
| Interpolation or vertical support | In-domain values, each typed boundary status, forward/backward replay |
| Integrator | Convergence case, signed-time symmetry, pole/dateline handling, abnormal classification |
| Boundary | Exact intersection, ordering, reflection limit, monotonic lifecycle output |
| Domain fill | Layer mass, residual carry, boundary inflow direction, stable birth identity |
| Query performance | Exact-key audit, execute-phase I/O counters, output digest, scaling matrix |

Real-data tests should use the smallest fixture that still exercises the
production reader and relevant topology. Performance work is accepted only
after normalized outputs remain consistent with the intended scientific change.
