# Windows native-netcdf environment (GNU host)

Recorded on 2026-07-14 for Trajecta M2 dual-backend NetCDF verification.

## Toolchain

| Component | Value |
|---|---|
| Rust host/target | `x86_64-pc-windows-gnu` (rustc 1.95.0) |
| C compiler | MSYS2 UCRT64 `gcc` 15.2.0 (`x86_64-w64-mingw32`) |
| netCDF-C | MSYS2 package `mingw-w64-ucrt-x86_64-netcdf` **4.9.3** |
| HDF5 | MSYS2 dependency of netcdf (UCRT64) |
| zlib | MSYS2 UCRT64 **1.3.1** (`libz.a` / `libz.dll.a`) |
| pkg-config | MSYS2 `mingw-w64-ucrt-x86_64-pkgconf` |
| ecCodes (optional combined) | project-local `.native/eccodes` (conda-style layout) |
| libclang (bindgen for eccodes-sys rebuild) | LLVM/Clang `bin` on `PATH`, or `LIBCLANG_PATH` |

Install (MSYS2 UCRT64):

```bash
pacman -S --needed mingw-w64-ucrt-x86_64-netcdf mingw-w64-ucrt-x86_64-pkgconf
```

If rebuilding `eccodes-sys` (for example after `cargo clean` or a new target dir), also provide libclang:

```bash
# example: LLVM from WinGet / official installer
export LIBCLANG_PATH="/c/Program Files/LLVM/bin"
# or MSYS2
# pacman -S mingw-w64-ucrt-x86_64-clang
# export LIBCLANG_PATH="/d/msys64/ucrt64/bin"
```

## Environment variables

```bash
export PATH="/d/msys64/ucrt64/bin:$PATH"
export PKG_CONFIG_PATH="/d/msys64/ucrt64/lib/pkgconfig"
export NETCDF_DIR="/d/msys64/ucrt64"
export LIBRARY_PATH="/d/msys64/ucrt64/lib"
export CPATH="/d/msys64/ucrt64/include"
# optional isolated target dir
export CARGO_TARGET_DIR="target-native"
```

For combined `native-eccodes,native-netcdf` (in addition to the above):

```bash
export PATH="$(pwd)/.native/eccodes/Library/bin:/d/msys64/ucrt64/bin:$PATH"
export ECCODES_DEFINITION_PATH="$(pwd)/.native/eccodes/Library/share/eccodes/definitions"
export PKG_CONFIG_PATH="/d/msys64/ucrt64/lib/pkgconfig:$(pwd)/.native/eccodes/Library/lib/pkgconfig"
# required when eccodes-sys runs bindgen (find libclang.dll on the machine)
# example (conda package layout):
export LIBCLANG_PATH="/c/Users/dell/miniforge3/pkgs/libclang-22.1.8-default_h570ddc7_3/Library/bin"
# Do NOT also export CPATH to MSYS2 UCRT64 headers while bindgen uses this clang;
# mixed clang + mingw headers can fail. Leave CPATH unset for the combined feature build.
```

`PATH` must include `/d/msys64/ucrt64/bin` at **runtime** so `libnetcdf-*.dll` and friends resolve.

## Verified commands

```bash
cargo clippy --offline -p trajecta-met --all-targets --features native-netcdf -- -D warnings
cargo test --offline -p trajecta-met --features native-netcdf
# after LIBCLANG_PATH is set and eccodes is available:
cargo clippy --offline -p trajecta-met --all-targets --features native-eccodes,native-netcdf -- -D warnings
cargo test --offline -p trajecta-met --features native-eccodes,native-netcdf
```

## Notes

- Conda/MSVC `netcdf.lib` under miniforge is **not** linked against the GNU Rust ABI; do not mix with `x86_64-pc-windows-gnu`.
- Feature `native-netcdf` hard-fails without the C worker; it never silently calls the pure-Rust decoder.
- Native decode uses netCDF-C **time/level hyperslabs** (`start`/`count` per dimension), not whole-variable reads for field decode.
- Differential tolerances are fixed in `tests/real_netcdf_pipeline.rs` (not data-adaptive) and cover **all valid times** plus grid/vertical metadata.
