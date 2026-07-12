# Trajecta contract skeleton

## Authority

Each public Rust module is defined in one same-named `.rs` or `mod.rs` file.
That source file is the authoritative contract for the module and begins with
module-level rustdoc describing:

- its responsibility;
- invariants that callers may rely on;
- work that belongs to another module;
- whether an operation is deliberately unavailable in the skeleton.

Rustdoc and public signatures are kept together so that the compiler checks
the contract. Separate prose in `docs/` explains architecture and rationale,
but does not override a public Rust signature.

## Skeleton rules

1. Every public module and public item must have documentation.
2. `unsafe`, `todo!`, `unimplemented!`, `panic!`, `unwrap`, and `expect` are
   forbidden by workspace lints.
3. Scientific algorithms, file decoders, caches, and run loops that are not
   implemented return a typed `NotImplemented` error.
4. Query execution contracts do not permit hidden file or network I/O.
5. The MIT crates never depend on the GPL compatibility or reference trees.
6. Cross-crate dependencies remain one-way: CLI may depend on all three
   libraries, core may depend on met and case, met may depend on case, and case
   depends on no other Trajecta crate.
7. Public data layouts encode status separately from floating-point values;
   NaN and magic numbers are not status channels.
8. Contract changes require updating affected tests and architecture docs in
   the same change.

## Contract version

The code-contract version is `0`. It is independent from package versions and
matches the development-stage `schema_version: 0` Case format. A future v1
freeze may make stronger compatibility guarantees.
