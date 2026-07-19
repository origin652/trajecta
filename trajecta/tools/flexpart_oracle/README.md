# FLEXPART GPL oracle harness (A2)

**License boundary:** this directory is GPL-facing tooling. MIT crates must only
consume frozen JSON oracle artifacts; they must not link or embed FLEXPART.

## Frozen commit

```text
dace3affa2ba71677f12f3858b04aaf59f8ee51e
```

## Scripts

| Path | Role |
|---|---|
| `generate_queries.py` | Frozen query matrix; LF bytes + SHA self-check; terrain sites when NetCDF surface available |
| `generate_oracle_stub.py` | Schema-valid **failed** v1 probe only (always non-zero exit) |
| `run_oracle.sh` | Checkout lock, queries, toolchain probe; refuses fake **complete** oracles |

There is **no** `run_oracle.ps1` and **no** completed `src/` Fortran driver yet.

## Status meanings

- `complete` — only after real `interpol_wind` / `interpol_partoutput_val` records exist
- `failed` — schema-valid probe / toolchain failure with non-empty real `input.files`
- `partial` — reserved; not emitted by the skeleton

If real input files cannot be identified, scripts write `STATUS.txt` / skip notes
and **do not** forge oracle JSON with empty `files`.

## Next implementation work

1. GPL-side loaders (ecCodes / netCDF-C) independent of Trajecta readers  
2. Dual builds: `pressure_meter` and `ETA`  
3. Call real FLEXPART vertical + interpolation routines  
4. Emit schema-valid complete oracle JSON per family  
