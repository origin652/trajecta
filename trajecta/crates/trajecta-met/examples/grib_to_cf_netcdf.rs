//! Offline GRIB → classic CF-NetCDF3 converter (not used by cargo test).
//!
//! Deterministically rewrites real GRIB meteorology into classic NetCDF3 so the
//! pure-Rust NetCDF path can be exercised on true business values. Runtime
//! ingestion never invokes this tool.
//!
//! Usage:
//!   cargo run -p trajecta-met --example grib_to_cf_netcdf -- \
//!     --input path/to/file.grb2 --output out.nc --family era5_cf_pressure_netcdf
//!
//! For hybrid ERA5 GRIB, pass `--family era5_cf_hybrid_netcdf4` and convert the
//! classic output with nccopy to NetCDF-4 outside this binary.

#![allow(
    clippy::print_stdout,
    clippy::print_stderr,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::type_complexity,
    clippy::needless_borrows_for_generic_args
)]

use std::collections::BTreeMap;
use std::env;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use netcdf3::{DataSet, FileWriter, Version};
use trajecta_case::document::MeteorologyReaderBackend;
use trajecta_met::io::grib::GribReader;
use trajecta_met::io::reader::{DecodeRequest, MetReader, SourceIndex};
use trajecta_met::vertical::VerticalTopology;

fn main() -> ExitCode {
    let args = env::args().skip(1).collect::<Vec<_>>();
    let mut input = None;
    let mut output = None;
    let mut family = "era5_cf_pressure_netcdf".to_owned();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--input" => {
                i += 1;
                input = Some(PathBuf::from(&args[i]));
            }
            "--output" => {
                i += 1;
                output = Some(PathBuf::from(&args[i]));
            }
            "--family" => {
                i += 1;
                family = args[i].clone();
            }
            "--help" | "-h" => {
                eprintln!("usage: grib_to_cf_netcdf --input GRIB --output NC [--family NAME]");
                return ExitCode::from(2);
            }
            other => {
                eprintln!("unknown argument {other}");
                return ExitCode::from(2);
            }
        }
        i += 1;
    }
    let Some(input) = input else {
        eprintln!("--input is required");
        return ExitCode::from(2);
    };
    let Some(output) = output else {
        eprintln!("--output is required");
        return ExitCode::from(2);
    };
    if let Err(error) = convert(&input, &output, &family) {
        eprintln!("conversion failed: {error}");
        return ExitCode::FAILURE;
    }
    println!("wrote {}", output.display());
    ExitCode::SUCCESS
}

fn convert(input: &Path, output: &Path, family: &str) -> Result<(), String> {
    let reader = GribReader::new(MeteorologyReaderBackend::Rust);
    let index = reader
        .build_index(input)
        .map_err(|error| format!("build_index: {error:?}"))?;
    let grid = SourceIndex::grid_geometry(index.as_ref())
        .ok_or_else(|| "missing grid geometry".to_owned())?;
    // Prefer the same validity set as inspect/metadata: layered (hybrid/isobaric)
    // times first. Surface-only messages can carry a different reference clock on
    // some GRIB products and must not create empty CF time slots.
    let meta = reader
        .inspect(input)
        .map_err(|error| format!("inspect: {error:?}"))?;
    let valid_times = meta.valid_times;
    if valid_times.is_empty() {
        return Err("GRIB has no validity times".into());
    }

    let lats = (0..grid.ny)
        .map(|j| grid.latitude_origin_degrees + j as f64 * grid.latitude_spacing_degrees)
        .collect::<Vec<_>>();
    let lons = (0..grid.nx)
        .map(|i| grid.longitude_origin_degrees + i as f64 * grid.longitude_spacing_degrees)
        .collect::<Vec<_>>();

    let mut dataset = DataSet::new();
    dataset
        .add_fixed_dim("time", valid_times.len())
        .map_err(|e| format!("{e:?}"))?;
    dataset
        .add_fixed_dim("lat", grid.ny)
        .map_err(|e| format!("{e:?}"))?;
    dataset
        .add_fixed_dim("lon", grid.nx)
        .map_err(|e| format!("{e:?}"))?;

    let is_hybrid = matches!(
        SourceIndex::vertical_topology(index.as_ref()),
        Some(VerticalTopology::HybridPressure(_))
    );
    let level_name = if is_hybrid { "hybrid" } else { "level" };
    let levels_pa = match SourceIndex::vertical_topology(index.as_ref()) {
        Some(VerticalTopology::PressureLevels(levels)) => levels.pressure_pa.to_vec(),
        Some(VerticalTopology::HybridPressure(topology)) => topology
            .active_full_levels
            .iter()
            .map(|level| f64::from(*level))
            .collect::<Vec<_>>(),
        None => return Err("vertical topology missing".into()),
    };
    let nlev = levels_pa.len();
    dataset
        .add_fixed_dim(level_name, nlev)
        .map_err(|e| format!("{e:?}"))?;

    add_coord_var(
        &mut dataset,
        "time",
        &["time"],
        &[
            ("units", "seconds since 1970-01-01 00:00:00"),
            ("calendar", "proleptic_gregorian"),
            ("axis", "T"),
            ("standard_name", "time"),
        ],
    )?;
    if is_hybrid {
        add_coord_var(
            &mut dataset,
            level_name,
            &[level_name],
            &[
                ("units", "1"),
                ("axis", "Z"),
                (
                    "standard_name",
                    "atmosphere_hybrid_sigma_pressure_coordinate",
                ),
                ("positive", "down"),
                ("formula_terms", "ap: ap b: b ps: sp"),
            ],
        )?;
        // Interface coefficients for hybrid.
        if let Some(VerticalTopology::HybridPressure(topology)) =
            SourceIndex::vertical_topology(index.as_ref())
        {
            let niface = topology.coefficients.a_half_pa.len();
            dataset
                .add_fixed_dim("nhyi", niface)
                .map_err(|e| format!("{e:?}"))?;
            add_coord_var(
                &mut dataset,
                "ap",
                &["nhyi"],
                &[("units", "Pa"), ("long_name", "hybrid A coefficient")],
            )?;
            add_coord_var(
                &mut dataset,
                "b",
                &["nhyi"],
                &[("units", "1"), ("long_name", "hybrid B coefficient")],
            )?;
        }
    } else {
        add_coord_var(
            &mut dataset,
            level_name,
            &[level_name],
            &[
                ("units", "Pa"),
                ("axis", "Z"),
                ("standard_name", "air_pressure"),
                ("positive", "down"),
            ],
        )?;
    }
    add_coord_var(
        &mut dataset,
        "lat",
        &["lat"],
        &[
            ("units", "degrees_north"),
            ("axis", "Y"),
            ("standard_name", "latitude"),
        ],
    )?;
    add_coord_var(
        &mut dataset,
        "lon",
        &["lon"],
        &[
            ("units", "degrees_east"),
            ("axis", "X"),
            ("standard_name", "longitude"),
        ],
    )?;

    // Transport variables.
    let layered_dims = ["time", level_name, "lat", "lon"];
    let surface_dims = ["time", "lat", "lon"];
    for (name, units, cf_name, is_standard_name, layered) in [
        ("t", "K", "air_temperature", true, true),
        ("u", "m/s", "eastward_wind", true, true),
        ("v", "m/s", "northward_wind", true, true),
        ("q", "1", "specific_humidity", true, true),
        (
            if is_hybrid { "etadot" } else { "w" },
            if is_hybrid { "s-1" } else { "Pa/s" },
            if is_hybrid {
                "hybrid coordinate vertical velocity"
            } else {
                "lagrangian_tendency_of_air_pressure"
            },
            !is_hybrid,
            true,
        ),
        ("sp", "Pa", "surface_air_pressure", true, false),
        ("z", "m2/s2", "surface_geopotential", true, false),
    ] {
        let dims: &[&str] = if layered {
            &layered_dims
        } else {
            &surface_dims
        };
        dataset
            .add_var_f64(name, dims)
            .map_err(|e| format!("{e:?}"))?;
        dataset
            .add_var_attr_string(name, "units", units)
            .map_err(|e| format!("{e:?}"))?;
        if is_standard_name {
            dataset
                .add_var_attr_string(name, "standard_name", cf_name)
                .map_err(|e| format!("{e:?}"))?;
        } else {
            dataset
                .add_var_attr_string(name, "long_name", cf_name)
                .map_err(|e| format!("{e:?}"))?;
        }
    }
    dataset
        .add_global_attr_string("Conventions", "CF-1.7")
        .map_err(|e| format!("{e:?}"))?;
    dataset
        .add_global_attr_string("dataset_family", family)
        .map_err(|e| format!("{e:?}"))?;
    dataset
        .add_global_attr_string(
            "history",
            &format!(
                "offline grib_to_cf_netcdf from {}",
                input.file_name().and_then(|s| s.to_str()).unwrap_or("grib")
            ),
        )
        .map_err(|e| format!("{e:?}"))?;

    if output.exists() {
        std::fs::remove_file(output).map_err(|e| format!("{e:?}"))?;
    }
    let mut writer = FileWriter::create_new(output).map_err(|e| format!("{e:?}"))?;
    writer
        .set_def(&dataset, Version::Classic, 0)
        .map_err(|e| format!("{e:?}"))?;

    let time_values = valid_times
        .iter()
        .map(|t| t.seconds_since_unix_epoch() as f64)
        .collect::<Vec<_>>();
    writer
        .write_var_f64("time", &time_values)
        .map_err(|e| format!("{e:?}"))?;
    writer
        .write_var_f64(level_name, &levels_pa)
        .map_err(|e| format!("{e:?}"))?;
    if is_hybrid {
        if let Some(VerticalTopology::HybridPressure(topology)) =
            SourceIndex::vertical_topology(index.as_ref())
        {
            writer
                .write_var_f64("ap", topology.coefficients.a_half_pa.as_ref())
                .map_err(|e| format!("{e:?}"))?;
            writer
                .write_var_f64("b", topology.coefficients.b_half.as_ref())
                .map_err(|e| format!("{e:?}"))?;
        }
    }
    writer
        .write_var_f64("lat", &lats)
        .map_err(|e| format!("{e:?}"))?;
    writer
        .write_var_f64("lon", &lons)
        .map_err(|e| format!("{e:?}"))?;

    // Decode fields per time. Pressure uses isobaric; hybrid uses hybrid.
    let horizontal = grid.nx.checked_mul(grid.ny).ok_or("grid overflow")?;
    let field_specs: &[(&str, Vec<(&str, String)>, bool)] = if is_hybrid {
        &[
            (
                "t",
                vec![
                    ("param_id", "130".into()),
                    ("type_of_level", "hybrid".into()),
                ],
                true,
            ),
            (
                "u",
                vec![
                    ("param_id", "131".into()),
                    ("type_of_level", "hybrid".into()),
                ],
                true,
            ),
            (
                "v",
                vec![
                    ("param_id", "132".into()),
                    ("type_of_level", "hybrid".into()),
                ],
                true,
            ),
            (
                "q",
                vec![
                    ("param_id", "133".into()),
                    ("type_of_level", "hybrid".into()),
                ],
                true,
            ),
            (
                "etadot",
                vec![
                    ("param_id", "77".into()),
                    ("type_of_level", "hybrid".into()),
                ],
                true,
            ),
            (
                "sp",
                vec![
                    ("param_id", "134".into()),
                    ("type_of_level", "surface".into()),
                ],
                false,
            ),
            (
                "z",
                vec![
                    ("param_id", "129".into()),
                    ("type_of_level", "surface".into()),
                ],
                false,
            ),
        ]
    } else {
        &[
            (
                "t",
                vec![
                    ("discipline", "0".into()),
                    ("parameter_category", "0".into()),
                    ("parameter_number", "0".into()),
                    ("type_of_level", "isobaric".into()),
                ],
                true,
            ),
            (
                "u",
                vec![
                    ("discipline", "0".into()),
                    ("parameter_category", "2".into()),
                    ("parameter_number", "2".into()),
                    ("type_of_level", "isobaric".into()),
                ],
                true,
            ),
            (
                "v",
                vec![
                    ("discipline", "0".into()),
                    ("parameter_category", "2".into()),
                    ("parameter_number", "3".into()),
                    ("type_of_level", "isobaric".into()),
                ],
                true,
            ),
            (
                "q",
                vec![
                    ("discipline", "0".into()),
                    ("parameter_category", "1".into()),
                    ("parameter_number", "0".into()),
                    ("type_of_level", "isobaric".into()),
                ],
                true,
            ),
            (
                "w",
                vec![
                    ("discipline", "0".into()),
                    ("parameter_category", "2".into()),
                    ("parameter_number", "8".into()),
                    ("type_of_level", "isobaric".into()),
                ],
                true,
            ),
            (
                "sp",
                vec![
                    ("discipline", "0".into()),
                    ("parameter_category", "3".into()),
                    ("parameter_number", "0".into()),
                    ("type_of_level", "surface".into()),
                ],
                false,
            ),
            (
                "z",
                // surface geopotential may be derived from orography height in CFSR;
                // request surface geopotential if present, else orography (height).
                vec![
                    ("discipline", "0".into()),
                    ("parameter_category", "3".into()),
                    ("parameter_number", "5".into()),
                    ("type_of_level", "surface".into()),
                ],
                false,
            ),
        ]
    };

    let mut buffers: BTreeMap<&str, Vec<f64>> = BTreeMap::new();
    for (name, _, layered) in field_specs {
        let per_time = if *layered {
            nlev * horizontal
        } else {
            horizontal
        };
        buffers.insert(*name, Vec::with_capacity(valid_times.len() * per_time));
    }

    // Decode each structural validity time separately. Never reuse one decode
    // across multiple CF time indices.
    for valid_time in &valid_times {
        for (name, identity, layered) in field_specs {
            let request = DecodeRequest {
                source_identity: identity
                    .iter()
                    .map(|(k, v)| ((*k).to_owned(), v.clone()))
                    .collect(),
                valid_time: Some(*valid_time),
            };
            let mut decoded = reader
                .decode(input, index.as_ref(), &request)
                .map_err(|error| format!("decode {name} at {valid_time:?}: {error:?}"))?;
            // CFSR orography height (m) → geopotential for surface z.
            if *name == "z" && !is_hybrid {
                let unit = decoded.source_unit.symbol();
                if unit == "m" {
                    let g0 = 9.806_65;
                    let values = Arc::make_mut(&mut decoded.values);
                    for value in values.iter_mut() {
                        *value *= g0;
                    }
                }
            }
            let expected = if *layered {
                nlev * horizontal
            } else {
                horizontal
            };
            if decoded.values.len() != expected {
                return Err(format!(
                    "{name} decoded len {} != expected {expected}",
                    decoded.values.len()
                ));
            }
            buffers
                .get_mut(name)
                .expect("buffer")
                .extend_from_slice(decoded.values.as_ref());
        }
    }

    for (name, values) in &buffers {
        writer
            .write_var_f64(name, values)
            .map_err(|e| format!("write {name}: {e:?}"))?;
    }
    writer.close().map_err(|e| format!("{e:?}"))?;
    Ok(())
}

fn add_coord_var(
    dataset: &mut DataSet,
    name: &str,
    dims: &[&str],
    attrs: &[(&str, &str)],
) -> Result<(), String> {
    dataset
        .add_var_f64(name, dims)
        .map_err(|e| format!("{e:?}"))?;
    for (key, value) in attrs {
        dataset
            .add_var_attr_string(name, key, value)
            .map_err(|e| format!("{e:?}"))?;
    }
    Ok(())
}
