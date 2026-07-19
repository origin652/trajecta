//! Independent CFSR q/omega full-field ULP measurement (does NOT touch registry).
//!
//! ```text
//! cargo run --offline -p trajecta-met --features native-eccodes --example measure_cfsr_ulp_calibration -- \
//!   out.json file1.grb2 file2.grb2 file3.grb2
//! ```
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::print_stdout,
    clippy::print_stderr
)]

use std::process::ExitCode;

#[cfg(feature = "native-eccodes")]
mod run {
    use std::env;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::ExitCode;

    use serde_json::{Value, json};
    use sha2::{Digest, Sha256};
    use trajecta_case::document::MeteorologyReaderBackend;
    use trajecta_met::io::grib::GribReader;
    use trajecta_met::io::reader::{DecodeRequest, MetReader};
    use trajecta_met::validation::tolerance::ordered_ulp_distance;

    fn sha256_file(path: &Path) -> String {
        let mut h = Sha256::new();
        h.update(fs::read(path).unwrap_or_default());
        h.finalize().iter().map(|b| format!("{b:02x}")).collect()
    }

    fn file_meta(path: &Path) -> Value {
        json!({
            "path": path.display().to_string(),
            "name": path.file_name().and_then(|s| s.to_str()).unwrap_or(""),
            "size": path.metadata().map(|m| m.len()).unwrap_or(0),
            "sha256": sha256_file(path),
        })
    }

    struct Worst {
        ulps: u64,
        abs: f64,
        idx: usize,
        rust: f64,
        native: f64,
        time: i64,
        file: String,
        field: String,
    }

    fn scan_field(path: &Path, token: &str) -> Result<Value, String> {
        let body = token.strip_prefix("grib:").unwrap_or(token);
        let (left, level) = body
            .split_once(':')
            .ok_or_else(|| format!("bad token {token}"))?;
        let bits: Vec<&str> = left.split('.').collect();
        if bits.len() != 3 {
            return Err(format!("bad id {left}"));
        }
        let rust = GribReader::new(MeteorologyReaderBackend::Rust);
        let native = GribReader::new(MeteorologyReaderBackend::Native);
        let rm = rust.inspect(path).map_err(|e| format!("{e:?}"))?;
        let ri = rust.build_index(path).map_err(|e| format!("{e:?}"))?;
        let ni = native.build_index(path).map_err(|e| format!("{e:?}"))?;
        let times = if rm.valid_times.is_empty() {
            vec![None]
        } else {
            rm.valid_times.iter().copied().map(Some).collect()
        };

        let mut compared = 0u64;
        let mut exact_mismatch = 0u64;
        let mut nonfinite = 0u64;
        let mut gt2 = 0u64;
        let mut max_abs = 0.0f64;
        let mut max_ulps = 0u64;
        let mut worst: Option<Worst> = None;
        let mut gt2_samples = Vec::new();

        for vt in times {
            let req = DecodeRequest {
                source_identity: vec![
                    ("discipline".into(), bits[0].into()),
                    ("parameter_category".into(), bits[1].into()),
                    ("parameter_number".into(), bits[2].into()),
                    ("type_of_level".into(), level.into()),
                ],
                valid_time: vt,
            };
            let rf = rust
                .decode(path, ri.as_ref(), &req)
                .map_err(|e| format!("rust {token}: {e:?}"))?;
            let nf = native
                .decode(path, ni.as_ref(), &req)
                .map_err(|e| format!("native {token}: {e:?}"))?;
            if rf.values.len() != nf.values.len() {
                return Err(format!(
                    "len mismatch {} vs {}",
                    rf.values.len(),
                    nf.values.len()
                ));
            }
            let tsecs = vt.map(|t| t.seconds_since_unix_epoch()).unwrap_or(0);
            let n = rf.values.len();
            // Infer 3D shape if possible: ny*nx from horizontal else linear only
            let (_levels, ny, nx) = match rf.layout {
                trajecta_met::frame::ArrayLayout::Full3D { levels, ny, nx } => (levels, ny, nx),
                trajecta_met::frame::ArrayLayout::Horizontal2D { ny, nx } => (1, ny, nx),
                _ => (1, 1, n),
            };
            for i in 0..n {
                let a = rf.values[i];
                let b = nf.values[i];
                if !a.is_finite() || !b.is_finite() {
                    nonfinite += 1;
                    continue;
                }
                compared += 1;
                if a != b {
                    exact_mismatch += 1;
                }
                let abs = (a - b).abs();
                if abs > max_abs {
                    max_abs = abs;
                }
                let ulps = ordered_ulp_distance(a, b);
                if ulps > max_ulps {
                    max_ulps = ulps;
                    worst = Some(Worst {
                        ulps,
                        abs,
                        idx: i,
                        rust: a,
                        native: b,
                        time: tsecs,
                        file: path
                            .file_name()
                            .and_then(|s| s.to_str())
                            .unwrap_or("")
                            .into(),
                        field: token.into(),
                    });
                }
                if ulps > 2 {
                    gt2 += 1;
                    if gt2_samples.len() < 32 {
                        let plane = ny.saturating_mul(nx);
                        let lev = i.checked_div(plane).unwrap_or(0);
                        let rem = if plane > 0 { i % plane } else { i };
                        let y = rem.checked_div(nx).unwrap_or(0);
                        let x = if nx > 0 { rem % nx } else { i };
                        gt2_samples.push(json!({
                            "linear_index": i,
                            "level_index": lev,
                            "y": y,
                            "x": x,
                            "ulps": ulps,
                            "abs": abs,
                            "rust": a,
                            "native": b,
                            "rust_bits_hex": format!("0x{:016x}", a.to_bits()),
                            "native_bits_hex": format!("0x{:016x}", b.to_bits()),
                            "valid_time_unix": tsecs,
                        }));
                    }
                }
            }
        }

        let mut worst_json = Value::Null;
        if let Some(w) = worst {
            // recompute y/x/level from idx using last layout dims — re-decode layout
            let req = DecodeRequest {
                source_identity: vec![
                    ("discipline".into(), bits[0].into()),
                    ("parameter_category".into(), bits[1].into()),
                    ("parameter_number".into(), bits[2].into()),
                    ("type_of_level".into(), level.into()),
                ],
                valid_time: None,
            };
            let rf = rust.decode(path, ri.as_ref(), &req).ok();
            let (_levels, ny, nx) = rf
                .as_ref()
                .map(|f| match f.layout {
                    trajecta_met::frame::ArrayLayout::Full3D { levels, ny, nx } => (levels, ny, nx),
                    trajecta_met::frame::ArrayLayout::Horizontal2D { ny, nx } => (1usize, ny, nx),
                    _ => (1usize, 1usize, w.idx + 1),
                })
                .unwrap_or((1, 1, w.idx + 1));
            let plane = ny.saturating_mul(nx);
            let lev = w.idx.checked_div(plane).unwrap_or(0);
            let rem = if plane > 0 { w.idx % plane } else { w.idx };
            let y = rem.checked_div(nx).unwrap_or(0);
            let x = if nx > 0 { rem % nx } else { w.idx };
            worst_json = json!({
                "file": w.file,
                "field": w.field,
                "valid_time_unix": w.time,
                "linear_index": w.idx,
                "level_index": lev,
                "y": y,
                "x": x,
                "maximum_ulps": w.ulps,
                "maximum_absolute_difference": w.abs,
                "rust_value": w.rust,
                "native_value": w.native,
                "rust_bits_hex": format!("0x{:016x}", w.rust.to_bits()),
                "native_bits_hex": format!("0x{:016x}", w.native.to_bits()),
                "rust_bits_u64": w.rust.to_bits(),
                "native_bits_u64": w.native.to_bits(),
            });
        }

        Ok(json!({
            "file": path.display().to_string(),
            "field": token,
            "compared_count": compared,
            "exact_mismatch_count": exact_mismatch,
            "nonfinite_count": nonfinite,
            "maximum_absolute_difference": max_abs,
            "maximum_ulps": max_ulps,
            "count_ulps_gt_2": gt2,
            "worst": worst_json,
            "samples_ulps_gt_2_cap32": gt2_samples,
            "grib_packing_note": "template/bitsPerValue/ref/scales: capture via eccodes keys when available; values above are pure decoded f64 ULP evidence",
        }))
    }

    pub fn main() -> ExitCode {
        let mut args = env::args().skip(1).collect::<Vec<_>>();
        if args.len() < 2 {
            eprintln!("usage: measure_cfsr_ulp_calibration out.json f00.grb2 f06.grb2 f12.grb2");
            return ExitCode::FAILURE;
        }
        let out = PathBuf::from(args.remove(0));
        let files: Vec<PathBuf> = args.into_iter().map(PathBuf::from).collect();
        for f in &files {
            if !f.is_file() {
                eprintln!("missing {f:?}");
                return ExitCode::FAILURE;
            }
        }

        let fields = ["grib:0.1.0:isobaric", "grib:0.2.8:isobaric"]; // q, omega
        let mut field_reports = Vec::new();
        for f in &files {
            for token in fields {
                match scan_field(f, token) {
                    Ok(v) => field_reports.push(v),
                    Err(e) => {
                        eprintln!("{e}");
                        return ExitCode::FAILURE;
                    }
                }
            }
        }

        // Highlight A-reproduced q 3 ULP envelope if present
        let mut reproduced_a_q_3ulp = false;
        for r in &field_reports {
            if r["field"] == "grib:0.1.0:isobaric" && r["maximum_ulps"].as_u64().unwrap_or(0) >= 3 {
                reproduced_a_q_3ulp = true;
            }
        }

        let doc = json!({
            "schema_hint": "trajecta.m3.cfsr_ulp_raw_calibration/v1",
            "status": "raw_measurement_not_registry",
            "note": "Independent raw measurement for A. B must not edit M3_TOLERANCES.v1.json from this artifact.",
            "grib_reader_crate": "grib-reader",
            "grib_reader_version": "0.6.0",
            "eccodes_note": "native backend via trajecta native-eccodes feature / system ecCodes",
            "files": files.iter().map(|p| file_meta(p)).collect::<Vec<_>>(),
            "fields": field_reports,
            "a_review_targets": {
                "q_abs_example": 3.101_927_297_073_854e-25,
                "q_value_abs_example": 9e-10,
                "expected_max_ulps_q": 3,
            },
            "reproduced_max_ulps_ge_3_on_q": reproduced_a_q_3ulp,
        });
        let body = serde_json::to_vec_pretty(&doc).unwrap();
        let mut bytes = body;
        if !bytes.ends_with(b"\n") {
            bytes.push(b'\n');
        }
        if let Some(parent) = out.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let tmp = out.with_extension("json.part");
        fs::write(&tmp, &bytes).unwrap();
        let _ = fs::rename(&tmp, &out).or_else(|_| {
            fs::copy(&tmp, &out).map(|_| {
                let _ = fs::remove_file(&tmp);
            })
        });
        let sha = {
            let mut h = Sha256::new();
            h.update(&bytes);
            h.finalize()
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>()
        };
        // sidecar sha of on-disk bytes
        let on = fs::read(&out).unwrap();
        let sha_disk = {
            let mut h = Sha256::new();
            h.update(&on);
            h.finalize()
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>()
        };
        fs::write(out.with_extension("json.sha256"), format!("{sha_disk}\n")).unwrap();
        println!(
            "wrote {} sha={} reproduced_q_3ulp={}",
            out.display(),
            &sha_disk[..16],
            reproduced_a_q_3ulp
        );
        if sha != sha_disk {
            eprintln!("sha mismatch buffer vs disk");
            return ExitCode::FAILURE;
        }
        ExitCode::SUCCESS
    }
}

fn main() -> ExitCode {
    #[cfg(feature = "native-eccodes")]
    {
        run::main()
    }
    #[cfg(not(feature = "native-eccodes"))]
    {
        eprintln!("need --features native-eccodes");
        ExitCode::FAILURE
    }
}
