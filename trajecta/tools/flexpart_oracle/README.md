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
| `generate_queries.py` | Frozen query matrix; LF bytes + SHA self-check; terrain sites when surface/orog available |
| `generate_oracle_stub.py` | Schema-valid **failed** v1 probe only (always non-zero exit) |
| `build_pressure_meter_driver.sh` | pressure_meter modules from frozen FLEXPART checkout |
| `run_pressure_meter_oracle.py` | ERA5 pressure full 75-record oracle (`complete` when all scopes ok) |
| `cfsr_grib_to_pressure_netcdf.py` | GPL ecCodes CLI adapter: official CFSR PGBl → pressure-meter NetCDF |
| `run_cfsr_pressure_oracle.py` | CFSR pressure full 75-record oracle via adapter + pressure_meter binary |
| `build_eta_hybrid_driver.sh` | ETA build (`-DETA`) + `nm` + `compile_identity.txt` |
| `run_era5_hybrid_oracle.py` | ERA5 hybrid-137 full 75-record oracle |
| `oracle_calcpar_mod.f90` | GPL PBL bridge (scalev/obukhov/richardson) — real `ishf`/`zust`/`blh` path |
| `run_oracle.sh` | Convenience wrapper (checkout lock + pressure path); prefer per-family runners for complete artifacts |

MIT comparison (separate gate): `tools/run_m3_oracle_comparison.py`.

**Not build tools (retired, exit 2):** `_patch_*.py`, `_apply_p0_format_pbl.py`.
ETA/pressure drivers are first-class sources. Three-family entry: `run_oracle.sh`.

Explicit identity env (not a single metdata_format conflation):
- `TRAJECTA_ORACLE_SOURCE_FAMILY=era5|cfsr`
- `TRAJECTA_ORACLE_VERTICAL_COORDINATE=pressure|hybrid_eta`
- `TRAJECTA_ORACLE_PBL_HEIGHT_MODE=official_prescribed|richardson_diagnosed`

Frozen combos:
- ERA5 pressure: era5 + pressure + official_prescribed + `verttransform_gfs` (adapter)
- CFSR pressure: cfsr + pressure + official_prescribed + `verttransform_gfs` (adapter)
- ERA5 hybrid: era5 + hybrid_eta + richardson_diagnosed + `verttransform_ecmwf`

Schema `build_mode` remains `pressure_meter|eta`; adapter/3-axis identity lives in
`*_oracle_identity.json` sidecar and `harness_version` tokens.
No silent Richardson fallback.

There is no `run_oracle.ps1`. Fortran drivers under `src/` call frozen FLEXPART
`verttransform_ecmwf`, `interpol_wind`, `interpol_partoutput_val`, and for
surface_layer also `interpol_pbl` / `interpol_pbl_short` after `oracle_calcpar`.
Hybrid ETA uses `coord_ecmwf_mod::z_to_zeta` and native heights from `etauvheight`.
Compile identity records full flags including `-g`, `-UUSE_NCF`, `-UETA`/`-DETA`.

## Status meanings

- `complete` — all 75 frozen records scientifically resolved (`ok`)
- `failed` — schema-valid probe / toolchain failure with non-empty real `input.files`
- `partial` — real FLEXPART results exist, but frozen query coverage is incomplete

If real input files cannot be identified, scripts write `STATUS.txt` / skip notes
and **do not** forge oracle JSON with empty `files`.
