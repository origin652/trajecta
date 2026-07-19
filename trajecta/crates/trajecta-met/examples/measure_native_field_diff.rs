//! Structured rust vs native field measurement (no tolerance adjudication).
//!
//! ```text
//! cargo run --offline -p trajecta-met --features native-netcdf --example measure_native_field_diff -- \
//!   --format netcdf path/to/file.nc t,u,v out.json
//!
//! cargo run --offline -p trajecta-met --features native-eccodes --example measure_native_field_diff -- \
//!   --format grib path/to/file.grb2 0.2.2:isobaric,0.0.0:isobaric out.json
//! ```

use std::process::ExitCode;

#[cfg(any(feature = "native-netcdf", feature = "native-eccodes"))]
mod measure {
    use std::env;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::ExitCode;

    #[derive(Clone, Copy, Debug)]
    enum Format {
        Netcdf,
        Grib,
    }

    struct DiffStats {
        exact_mismatch: u64,
        nonfinite: u64,
        max_abs: f64,
        max_rel: f64,
        worst_abs_index: usize,
        worst_abs_rust: f64,
        worst_abs_native: f64,
        worst_rel_index: usize,
        worst_rel_rust: f64,
        worst_rel_native: f64,
        len_rust: usize,
        len_native: usize,
        layout_equal: bool,
        mask_equal: bool,
        unit_rust: String,
        unit_native: String,
    }

    fn diff_fields(
        rust_values: &[f64],
        native_values: &[f64],
        layout_equal: bool,
        mask_equal: bool,
        unit_rust: String,
        unit_native: String,
    ) -> DiffStats {
        let mut exact_mismatch = 0u64;
        let mut nonfinite = 0u64;
        let mut max_abs = 0.0_f64;
        let mut max_rel = 0.0_f64;
        let mut worst_abs_index = 0usize;
        let mut worst_abs_rust = 0.0_f64;
        let mut worst_abs_native = 0.0_f64;
        let mut worst_rel_index = 0usize;
        let mut worst_rel_rust = 0.0_f64;
        let mut worst_rel_native = 0.0_f64;
        let n = rust_values.len().min(native_values.len());
        for i in 0..n {
            let a = rust_values[i];
            let b = native_values[i];
            if !a.is_finite() || !b.is_finite() {
                nonfinite += 1;
                continue;
            }
            let abs = (a - b).abs();
            if abs > 0.0 {
                exact_mismatch += 1;
            }
            if abs > max_abs {
                max_abs = abs;
                worst_abs_index = i;
                worst_abs_rust = a;
                worst_abs_native = b;
            }
            // True max relative error, independent of max_abs.
            let denom = a.abs().max(b.abs()).max(1.0);
            let rel = abs / denom;
            if rel > max_rel {
                max_rel = rel;
                worst_rel_index = i;
                worst_rel_rust = a;
                worst_rel_native = b;
            }
        }
        if rust_values.len() != native_values.len() {
            exact_mismatch = exact_mismatch.saturating_add(1);
        }
        DiffStats {
            exact_mismatch,
            nonfinite,
            max_abs,
            max_rel,
            worst_abs_index,
            worst_abs_rust,
            worst_abs_native,
            worst_rel_index,
            worst_rel_rust,
            worst_rel_native,
            len_rust: rust_values.len(),
            len_native: native_values.len(),
            layout_equal,
            mask_equal,
            unit_rust,
            unit_native,
        }
    }

    fn stats_to_json(
        variable: &str,
        valid_time_unix: Option<i64>,
        stats: DiffStats,
        error: Option<String>,
    ) -> serde_json::Value {
        if let Some(error) = error {
            return serde_json::json!({
                "variable": variable,
                "valid_time_unix": valid_time_unix,
                "error": error,
            });
        }
        serde_json::json!({
            "variable": variable,
            "valid_time_unix": valid_time_unix,
            "len_rust": stats.len_rust,
            "len_native": stats.len_native,
            "layout_equal": stats.layout_equal,
            "mask_equal": stats.mask_equal,
            "unit_rust": stats.unit_rust,
            "unit_native": stats.unit_native,
            "exact_mismatch_count": stats.exact_mismatch,
            "nonfinite_count": stats.nonfinite,
            "max_abs": stats.max_abs,
            "max_rel": stats.max_rel,
            "worst_abs_index": stats.worst_abs_index,
            "worst_abs_rust": stats.worst_abs_rust,
            "worst_abs_native": stats.worst_abs_native,
            "worst_index": stats.worst_abs_index,
            "worst_rust": stats.worst_abs_rust,
            "worst_native": stats.worst_abs_native,
            "worst_rel_index": stats.worst_rel_index,
            "worst_rel_rust": stats.worst_rel_rust,
            "worst_rel_native": stats.worst_rel_native,
        })
    }

    fn parse_args() -> Result<(Format, PathBuf, Vec<String>, PathBuf), String> {
        let mut args = env::args().skip(1).collect::<Vec<_>>();
        let mut format = Format::Netcdf;
        if args.first().map(String::as_str) == Some("--format") {
            if args.len() < 2 {
                return Err("missing format after --format".into());
            }
            format = match args[1].as_str() {
                "netcdf" | "nc" => Format::Netcdf,
                "grib" | "grb" | "grb2" => Format::Grib,
                other => return Err(format!("unknown format {other}")),
            };
            args.drain(0..2);
        } else if let Some(path) = args.first() {
            let lower = path.to_ascii_lowercase();
            if lower.ends_with(".grb") || lower.ends_with(".grb2") || lower.ends_with(".grib2") {
                format = Format::Grib;
            }
        }
        if args.len() < 3 {
            return Err(
                "usage: measure_native_field_diff [--format netcdf|grib] <file> <vars> <out.json>"
                    .into(),
            );
        }
        let path = PathBuf::from(&args[0]);
        let vars = args[1]
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .collect::<Vec<_>>();
        let out = PathBuf::from(&args[2]);
        Ok((format, path, vars, out))
    }

    #[cfg(feature = "native-netcdf")]
    fn measure_netcdf(path: &Path, vars: &[String]) -> Result<Vec<serde_json::Value>, String> {
        use trajecta_case::document::MeteorologyReaderBackend;
        use trajecta_met::io::netcdf::NetCdfReader;
        use trajecta_met::io::reader::{DecodeRequest, MetReader};

        let rust = NetCdfReader::new(MeteorologyReaderBackend::Rust);
        let native = NetCdfReader::new(MeteorologyReaderBackend::Native);
        let rust_index = rust
            .build_index(path)
            .map_err(|e| format!("rust index: {e:?}"))?;
        let native_index = native
            .build_index(path)
            .map_err(|e| format!("native index: {e:?}"))?;
        let meta = rust.inspect(path).map_err(|e| format!("inspect: {e:?}"))?;

        let mut measurements = Vec::new();
        for valid_time in &meta.valid_times {
            for variable in vars {
                let request = DecodeRequest {
                    source_identity: vec![("variable".into(), variable.clone())],
                    valid_time: Some(*valid_time),
                };
                let unix = valid_time.seconds_since_unix_epoch();
                let rust_field = match rust.decode(path, rust_index.as_ref(), &request) {
                    Ok(v) => v,
                    Err(e) => {
                        measurements.push(stats_to_json(
                            variable,
                            Some(unix),
                            DiffStats {
                                exact_mismatch: 0,
                                nonfinite: 0,
                                max_abs: 0.0,
                                max_rel: 0.0,
                                worst_abs_index: 0,
                                worst_abs_rust: 0.0,
                                worst_abs_native: 0.0,
                                worst_rel_index: 0,
                                worst_rel_rust: 0.0,
                                worst_rel_native: 0.0,
                                len_rust: 0,
                                len_native: 0,
                                layout_equal: false,
                                mask_equal: false,
                                unit_rust: String::new(),
                                unit_native: String::new(),
                            },
                            Some(format!("rust decode: {e:?}")),
                        ));
                        continue;
                    }
                };
                let native_field = match native.decode(path, native_index.as_ref(), &request) {
                    Ok(v) => v,
                    Err(e) => {
                        measurements.push(stats_to_json(
                            variable,
                            Some(unix),
                            DiffStats {
                                exact_mismatch: 0,
                                nonfinite: 0,
                                max_abs: 0.0,
                                max_rel: 0.0,
                                worst_abs_index: 0,
                                worst_abs_rust: 0.0,
                                worst_abs_native: 0.0,
                                worst_rel_index: 0,
                                worst_rel_rust: 0.0,
                                worst_rel_native: 0.0,
                                len_rust: 0,
                                len_native: 0,
                                layout_equal: false,
                                mask_equal: false,
                                unit_rust: String::new(),
                                unit_native: String::new(),
                            },
                            Some(format!("native decode: {e:?}")),
                        ));
                        continue;
                    }
                };
                let stats = diff_fields(
                    &rust_field.values,
                    &native_field.values,
                    rust_field.layout == native_field.layout,
                    rust_field.valid == native_field.valid,
                    rust_field.source_unit.symbol().to_owned(),
                    native_field.source_unit.symbol().to_owned(),
                );
                measurements.push(stats_to_json(variable, Some(unix), stats, None));
            }
        }
        Ok(measurements)
    }

    #[cfg(not(feature = "native-netcdf"))]
    fn measure_netcdf(_path: &Path, _vars: &[String]) -> Result<Vec<serde_json::Value>, String> {
        Err("rebuild with --features native-netcdf".into())
    }

    #[cfg(feature = "native-eccodes")]
    fn measure_grib(path: &Path, vars: &[String]) -> Result<Vec<serde_json::Value>, String> {
        use trajecta_case::document::MeteorologyReaderBackend;
        use trajecta_met::io::grib::GribReader;
        use trajecta_met::io::reader::{DecodeRequest, MetReader};

        let rust = GribReader::new(MeteorologyReaderBackend::Rust);
        let native = GribReader::new(MeteorologyReaderBackend::Native);
        let rust_index = rust
            .build_index(path)
            .map_err(|e| format!("rust index: {e:?}"))?;
        let native_index = native
            .build_index(path)
            .map_err(|e| format!("native index: {e:?}"))?;
        let meta = rust.inspect(path).map_err(|e| format!("inspect: {e:?}"))?;
        let valid_times = if meta.valid_times.is_empty() {
            vec![None]
        } else {
            meta.valid_times.iter().copied().map(Some).collect()
        };

        let mut measurements = Vec::new();
        for valid_time in valid_times {
            for token in vars {
                let Some((left, level_type)) = token.split_once(':') else {
                    measurements.push(serde_json::json!({
                        "variable": token,
                        "error": "grib token must contain ':' separating identity and type_of_level",
                    }));
                    continue;
                };
                let source_identity = if left.contains('.') {
                    let bits: Vec<&str> = left.split('.').collect();
                    if bits.len() != 3 {
                        measurements.push(serde_json::json!({
                            "variable": token,
                            "error": "expected discipline.category.number:type_of_level",
                        }));
                        continue;
                    }
                    vec![
                        ("discipline".into(), bits[0].to_owned()),
                        ("parameter_category".into(), bits[1].to_owned()),
                        ("parameter_number".into(), bits[2].to_owned()),
                        ("type_of_level".into(), level_type.to_owned()),
                    ]
                } else {
                    vec![
                        ("param_id".into(), left.to_owned()),
                        ("type_of_level".into(), level_type.to_owned()),
                    ]
                };
                let request = DecodeRequest {
                    source_identity,
                    valid_time,
                };
                let unix = valid_time.map(|t| t.seconds_since_unix_epoch());
                let rust_field = match rust.decode(path, rust_index.as_ref(), &request) {
                    Ok(v) => v,
                    Err(e) => {
                        measurements.push(serde_json::json!({
                            "variable": token,
                            "valid_time_unix": unix,
                            "error": format!("rust decode: {e:?}"),
                        }));
                        continue;
                    }
                };
                let native_field = match native.decode(path, native_index.as_ref(), &request) {
                    Ok(v) => v,
                    Err(e) => {
                        measurements.push(serde_json::json!({
                            "variable": token,
                            "valid_time_unix": unix,
                            "error": format!("native decode: {e:?}"),
                        }));
                        continue;
                    }
                };
                let stats = diff_fields(
                    &rust_field.values,
                    &native_field.values,
                    rust_field.layout == native_field.layout,
                    rust_field.valid == native_field.valid,
                    rust_field.source_unit.symbol().to_owned(),
                    native_field.source_unit.symbol().to_owned(),
                );
                measurements.push(stats_to_json(token, unix, stats, None));
            }
        }
        Ok(measurements)
    }

    #[cfg(not(feature = "native-eccodes"))]
    fn measure_grib(_path: &Path, _vars: &[String]) -> Result<Vec<serde_json::Value>, String> {
        Err("rebuild with --features native-eccodes".into())
    }

    pub fn run() -> ExitCode {
        let (format, path, vars, out) = match parse_args() {
            Ok(v) => v,
            Err(e) => {
                eprintln!("{e}");
                return ExitCode::FAILURE;
            }
        };

        let measurements = match format {
            Format::Netcdf => measure_netcdf(&path, &vars),
            Format::Grib => measure_grib(&path, &vars),
        };
        let measurements = match measurements {
            Ok(v) => v,
            Err(e) => {
                eprintln!("{e}");
                return ExitCode::FAILURE;
            }
        };

        let payload = serde_json::json!({
            "status": "unvalidated_measurement",
            "kind": "rust_vs_native_field_diff",
            "format": match format {
                Format::Netcdf => "netcdf",
                Format::Grib => "grib",
            },
            "file": path.display().to_string(),
            "variables": vars,
            "measurements": measurements,
            "notes": [
                "No tolerance pass/fail is asserted.",
                "exact_mismatch_count counts positions with abs(rust-native)>0 among finite pairs.",
                "max_rel is the true maximum relative error (denom = max(|a|,|b|,1)), independent of max_abs.",
                "worst_index/worst_rust/worst_native alias worst_abs_*.",
            ],
        });
        let Ok(body) = serde_json::to_string_pretty(&payload) else {
            eprintln!("serialize failed");
            return ExitCode::FAILURE;
        };
        if fs::write(&out, body + "\n").is_err() {
            eprintln!("write failed");
            return ExitCode::FAILURE;
        }
        println!("wrote {}", out.display());
        ExitCode::SUCCESS
    }
}

fn main() -> ExitCode {
    #[cfg(any(feature = "native-netcdf", feature = "native-eccodes"))]
    {
        measure::run()
    }
    #[cfg(not(any(feature = "native-netcdf", feature = "native-eccodes")))]
    {
        eprintln!("rebuild with --features native-netcdf and/or native-eccodes");
        ExitCode::FAILURE
    }
}
