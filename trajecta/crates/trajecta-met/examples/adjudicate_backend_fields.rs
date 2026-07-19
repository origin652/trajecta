//! Full-field backend adjudication against frozen M3_TOLERANCES.v1.json.
//!
//! Specs use `::` separators (Windows-safe):
//!   path::pressure::t,u,v
//!   path::grib::0.2.2:isobaric,0.0.0:isobaric
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::print_stdout,
    clippy::print_stderr
)]

use std::process::ExitCode;

#[cfg(any(feature = "native-netcdf", feature = "native-eccodes"))]
mod run {
    use std::collections::BTreeSet;
    use std::env;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::ExitCode;

    use sha2::{Digest, Sha256};
    use trajecta_met::frame::{ArrayLayout, TemporalSupport};
    use trajecta_met::io::reader::{DecodeRequest, DecodedField, MetReader, SourceMetadata};
    use trajecta_met::validation::report::{
        ComparisonIdentity, ComparisonStatus, FieldSlab, SampleContextOwned, SideMetadata,
        adjudicate, report_to_json,
    };
    use trajecta_met::validation::tolerance::load_registry_from_path;

    fn sha256_file(path: &Path) -> String {
        let bytes = fs::read(path).unwrap_or_default();
        let mut h = Sha256::new();
        h.update(&bytes);
        h.finalize().iter().map(|b| format!("{b:02x}")).collect()
    }

    fn sha256_bytes(bytes: &[u8]) -> String {
        let mut h = Sha256::new();
        h.update(bytes);
        h.finalize().iter().map(|b| format!("{b:02x}")).collect()
    }

    fn layout_sig(layout: &ArrayLayout) -> String {
        match *layout {
            ArrayLayout::Scalar => "scalar".into(),
            ArrayLayout::Horizontal2D { ny, nx } => format!("h2d:{ny}x{nx}"),
            ArrayLayout::Full3D { levels, ny, nx } => format!("full3d:{levels}x{ny}x{nx}"),
            ArrayLayout::Interface3D { levels, ny, nx } => format!("iface3d:{levels}x{ny}x{nx}"),
        }
    }

    fn temporal_sig(t: &TemporalSupport) -> String {
        match *t {
            TemporalSupport::Instantaneous { valid_time } => {
                format!("instant:{}", valid_time.seconds_since_unix_epoch())
            }
            TemporalSupport::Interval { start, end } => format!(
                "interval:{}:{}",
                start.seconds_since_unix_epoch(),
                end.seconds_since_unix_epoch()
            ),
            TemporalSupport::Accumulation { reset, start, end } => format!(
                "accum:{}:{}:{}",
                reset.seconds_since_unix_epoch(),
                start.seconds_since_unix_epoch(),
                end.seconds_since_unix_epoch()
            ),
        }
    }

    fn grid_sig(meta: &SourceMetadata) -> String {
        match &meta.grid {
            Some(g) => format!(
                "nx={}ny={}period={}sha={}",
                g.nx, g.ny, g.periodic_longitude, g.sha256
            ),
            None => {
                let mut dims: Vec<_> = meta.dimensions.iter().collect();
                dims.sort_by(|a, b| a.0.cmp(b.0));
                format!("dims:{dims:?}")
            }
        }
    }

    fn vertical_sig(meta: &SourceMetadata) -> String {
        match &meta.vertical {
            Some(v) => format!("{v:?}"),
            None => "none".into(),
        }
    }

    fn element_count(layout: &ArrayLayout) -> usize {
        match *layout {
            ArrayLayout::Scalar => 1,
            ArrayLayout::Horizontal2D { ny, nx } => ny.saturating_mul(nx),
            ArrayLayout::Full3D { levels, ny, nx }
            | ArrayLayout::Interface3D { levels, ny, nx } => {
                levels.saturating_mul(ny).saturating_mul(nx)
            }
        }
    }

    fn side_meta(src: &SourceMetadata, field: &DecodedField, valid_time_secs: i64) -> SideMetadata {
        SideMetadata {
            valid_time_unix_seconds: valid_time_secs,
            valid_time_nanosecond: 0,
            grid_signature: grid_sig(src),
            vertical_signature: vertical_sig(src),
            layout_signature: layout_sig(&field.layout),
            unit: field.source_unit.symbol().to_owned(),
            temporal_support: temporal_sig(&field.temporal),
            status: Some("ok".into()),
        }
    }

    #[cfg(feature = "native-netcdf")]
    fn decode_netcdf_pair(
        path: &Path,
        variable: &str,
        valid_time: trajecta_case::model::time::Timestamp,
    ) -> Result<(DecodedField, DecodedField, SourceMetadata, SourceMetadata), String> {
        use trajecta_case::document::MeteorologyReaderBackend;
        use trajecta_met::io::netcdf::NetCdfReader;
        let rust = NetCdfReader::new(MeteorologyReaderBackend::Rust);
        let native = NetCdfReader::new(MeteorologyReaderBackend::Native);
        let rm = rust
            .inspect(path)
            .map_err(|e| format!("rust inspect: {e:?}"))?;
        let nm = native
            .inspect(path)
            .map_err(|e| format!("native inspect: {e:?}"))?;
        let ri = rust.build_index(path).map_err(|e| format!("{e:?}"))?;
        let ni = native.build_index(path).map_err(|e| format!("{e:?}"))?;
        let req = DecodeRequest {
            source_identity: vec![("variable".into(), variable.into())],
            valid_time: Some(valid_time),
        };
        let rf = rust
            .decode(path, ri.as_ref(), &req)
            .map_err(|e| format!("rust {variable}: {e:?}"))?;
        let nf = native
            .decode(path, ni.as_ref(), &req)
            .map_err(|e| format!("native {variable}: {e:?}"))?;
        Ok((rf, nf, rm, nm))
    }

    #[cfg(feature = "native-eccodes")]
    fn decode_grib_pair(
        path: &Path,
        token: &str,
        valid_time: Option<trajecta_case::model::time::Timestamp>,
    ) -> Result<(DecodedField, DecodedField, SourceMetadata, SourceMetadata), String> {
        use trajecta_case::document::MeteorologyReaderBackend;
        use trajecta_met::io::grib::GribReader;
        let body = token.strip_prefix("grib:").unwrap_or(token);
        let (left, level) = body
            .split_once(':')
            .ok_or_else(|| format!("bad grib token {token}"))?;
        let bits: Vec<&str> = left.split('.').collect();
        if bits.len() != 3 {
            return Err(format!("bad grib identity {left}"));
        }
        let rust = GribReader::new(MeteorologyReaderBackend::Rust);
        let native = GribReader::new(MeteorologyReaderBackend::Native);
        let rm = rust
            .inspect(path)
            .map_err(|e| format!("rust inspect: {e:?}"))?;
        let nm = native
            .inspect(path)
            .map_err(|e| format!("native inspect: {e:?}"))?;
        let ri = rust.build_index(path).map_err(|e| format!("{e:?}"))?;
        let ni = native.build_index(path).map_err(|e| format!("{e:?}"))?;
        let req = DecodeRequest {
            source_identity: vec![
                ("discipline".into(), bits[0].into()),
                ("parameter_category".into(), bits[1].into()),
                ("parameter_number".into(), bits[2].into()),
                ("type_of_level".into(), level.into()),
            ],
            valid_time,
        };
        let rf = rust
            .decode(path, ri.as_ref(), &req)
            .map_err(|e| format!("rust {token}: {e:?}"))?;
        let nf = native
            .decode(path, ni.as_ref(), &req)
            .map_err(|e| format!("native {token}: {e:?}"))?;
        Ok((rf, nf, rm, nm))
    }

    #[allow(clippy::too_many_arguments)]
    fn make_slab(
        slab_id: String,
        family: &str,
        variant: &str,
        field_id: &str,
        vt_secs: i64,
        rf: &DecodedField,
        nf: &DecodedField,
        rm: &SourceMetadata,
        nm: &SourceMetadata,
    ) -> Result<FieldSlab, String> {
        let exp = element_count(&rf.layout);
        if element_count(&nf.layout) != exp {
            return Err(format!(
                "{slab_id}: layout element count mismatch rust={exp} native={}",
                element_count(&nf.layout)
            ));
        }
        if rf.values.len() != exp || nf.values.len() != exp {
            return Err(format!(
                "{slab_id}: value len mismatch rust={} native={} expected={exp}",
                rf.values.len(),
                nf.values.len()
            ));
        }
        if rf.valid.len() != exp || nf.valid.len() != exp {
            return Err(format!("{slab_id}: mask len mismatch"));
        }
        let element_case_ids = (0..exp)
            .map(|i| format!("{slab_id}#{i}"))
            .collect::<Vec<_>>();
        Ok(FieldSlab {
            slab_id,
            context: SampleContextOwned {
                dataset_family: family.into(),
                comparison_target: "backend_equivalence".into(),
                comparison_variant: variant.into(),
                sample_scope: "decoded_field".into(),
                field_namespace: "source_identity".into(),
                field: field_id.into(),
                coordinate: "grid".into(),
                vertical_region: "column".into(),
            },
            subject_meta: side_meta(rm, rf, vt_secs),
            reference_meta: side_meta(nm, nf, vt_secs),
            expected_element_count: exp,
            subject_values: rf.values.to_vec(),
            reference_values: nf.values.to_vec(),
            subject_mask: rf.valid.to_vec(),
            reference_mask: nf.valid.to_vec(),
            element_case_ids,
            vector_elements: None,
        })
    }

    fn verify_frozen_file(root: &Path, entry: &serde_json::Value) -> Result<PathBuf, String> {
        let rel = entry
            .get("relative_path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "manifest file missing relative_path".to_string())?;
        let path = root.join(rel);
        if !path.is_file() {
            return Err(format!("missing_file:{rel}"));
        }
        let size = path.metadata().map(|m| m.len()).unwrap_or(0);
        let exp_size = entry.get("size").and_then(|v| v.as_u64()).unwrap_or(0);
        if size != exp_size {
            return Err(format!("size_mismatch:{rel}:disk={size}:frozen={exp_size}"));
        }
        let digest = sha256_file(&path);
        let exp_sha = entry.get("sha256").and_then(|v| v.as_str()).unwrap_or("");
        if digest != exp_sha {
            return Err(format!("sha_mismatch:{rel}:disk={digest}:frozen={exp_sha}"));
        }
        Ok(path)
    }

    pub fn main() -> ExitCode {
        let args = env::args().skip(1).collect::<Vec<_>>();
        let mut format = "netcdf".to_string();
        let mut family = String::new();
        let mut variant = String::new();
        let mut registry = PathBuf::from("testdata/M3_TOLERANCES.v1.json");
        let mut out = PathBuf::from("report.json");
        let mut report_id = "backend/adjudicate/v1".to_string();
        let mut manifest_path: Option<PathBuf> = None;
        let mut manifest_family = String::new();
        let mut i = 0;
        while i < args.len() {
            match args[i].as_str() {
                "--format" => {
                    format = args.get(i + 1).cloned().unwrap_or_default();
                    i += 2;
                }
                "--family" => {
                    family = args.get(i + 1).cloned().unwrap_or_default();
                    i += 2;
                }
                "--variant" => {
                    variant = args.get(i + 1).cloned().unwrap_or_default();
                    i += 2;
                }
                "--registry" => {
                    registry = PathBuf::from(args.get(i + 1).cloned().unwrap_or_default());
                    i += 2;
                }
                "--out" => {
                    out = PathBuf::from(args.get(i + 1).cloned().unwrap_or_default());
                    i += 2;
                }
                "--report-id" => {
                    report_id = args.get(i + 1).cloned().unwrap_or_default();
                    i += 2;
                }
                "--manifest" => {
                    manifest_path =
                        Some(PathBuf::from(args.get(i + 1).cloned().unwrap_or_default()));
                    i += 2;
                }
                "--manifest-family" => {
                    manifest_family = args.get(i + 1).cloned().unwrap_or_default();
                    i += 2;
                }
                "--expected-times-per-file" => {
                    // deprecated: times come from frozen manifest
                    i += 2;
                }
                _ => break,
            }
        }

        let Some(manifest_path) = manifest_path else {
            eprintln!(
                "usage: adjudicate_backend_fields --manifest MATRIX.json --manifest-family F ..."
            );
            return ExitCode::from(2);
        };
        if manifest_family.is_empty() {
            eprintln!("--manifest-family required");
            return ExitCode::from(2);
        }

        let manifest_raw = match fs::read_to_string(&manifest_path) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("read manifest: {e}");
                return ExitCode::from(2);
            }
        };
        let manifest: serde_json::Value = match serde_json::from_str(&manifest_raw) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("parse manifest: {e}");
                return ExitCode::from(2);
            }
        };
        let fam_cfg = match manifest
            .pointer(&format!("/families/{manifest_family}"))
            .cloned()
        {
            Some(v) => v,
            None => {
                eprintln!("family {manifest_family} missing from manifest");
                return ExitCode::from(2);
            }
        };
        if family.is_empty() {
            family = manifest_family.clone();
        }
        if variant.is_empty() {
            variant = fam_cfg
                .get("variant")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_owned();
        }
        if report_id == "backend/adjudicate/v1" {
            if let Some(rid) = fam_cfg.get("report_id").and_then(|v| v.as_str()) {
                report_id = rid.to_owned();
            }
        }
        if let Some(fmt) = fam_cfg.get("format").and_then(|v| v.as_str()) {
            format = fmt.to_owned();
        }
        if family.is_empty() || variant.is_empty() {
            eprintln!("family/variant unresolved");
            return ExitCode::from(2);
        }

        let reg = match load_registry_from_path(&registry) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("{e}");
                return ExitCode::FAILURE;
            }
        };

        // Workspace root: parent of testdata/ or cwd.
        let root = manifest_path
            .parent()
            .and_then(|p| p.parent())
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));

        // Frozen expected matrix BEFORE decode: exact files × frozen times × frozen fields.
        let mut expected_case_ids = BTreeSet::new();
        let mut slabs = Vec::new();
        let mut file_digests = Vec::new();
        let mut blockers = Vec::new();

        let files = match fam_cfg.get("files").and_then(|v| v.as_array()) {
            Some(a) => a,
            None => {
                eprintln!("manifest family has no files");
                return ExitCode::from(2);
            }
        };

        for entry in files {
            let path = match verify_frozen_file(&root, entry) {
                Ok(p) => p,
                Err(e) => {
                    blockers.push(e);
                    continue;
                }
            };
            let fields: Vec<String> = entry
                .get("fields")
                .and_then(|v| v.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default();
            let times: Vec<i64> = entry
                .get("valid_times_unix")
                .and_then(|v| v.as_array())
                .map(|a| a.iter().filter_map(|x| x.as_i64()).collect())
                .unwrap_or_default();
            if fields.is_empty() || times.is_empty() {
                blockers.push(format!("empty_fields_or_times:{}", path.display()));
                continue;
            }

            // Cross-check inspect times equal frozen set (exact multiset), not mere count.
            let inspected_times: Result<Vec<i64>, String> = if format == "netcdf" {
                #[cfg(feature = "native-netcdf")]
                {
                    use trajecta_case::document::MeteorologyReaderBackend;
                    use trajecta_met::io::netcdf::NetCdfReader;
                    let rust = NetCdfReader::new(MeteorologyReaderBackend::Rust);
                    rust.inspect(&path)
                        .map(|m| {
                            m.valid_times
                                .iter()
                                .map(|t| t.seconds_since_unix_epoch())
                                .collect()
                        })
                        .map_err(|e| format!("inspect:{}:{e:?}", path.display()))
                }
                #[cfg(not(feature = "native-netcdf"))]
                {
                    Err("native-netcdf feature required".into())
                }
            } else {
                #[cfg(feature = "native-eccodes")]
                {
                    use trajecta_case::document::MeteorologyReaderBackend;
                    use trajecta_met::io::grib::GribReader;
                    let rust = GribReader::new(MeteorologyReaderBackend::Rust);
                    rust.inspect(&path)
                        .map(|m| {
                            m.valid_times
                                .iter()
                                .map(|t| t.seconds_since_unix_epoch())
                                .collect()
                        })
                        .map_err(|e| format!("inspect:{}:{e:?}", path.display()))
                }
                #[cfg(not(feature = "native-eccodes"))]
                {
                    Err("native-eccodes feature required".into())
                }
            };
            match inspected_times {
                Ok(got) => {
                    let mut a = got.clone();
                    let mut b = times.clone();
                    a.sort_unstable();
                    b.sort_unstable();
                    if a != b {
                        blockers.push(format!(
                            "valid_times_mismatch:{}:disk={got:?}:frozen={times:?}",
                            path.display()
                        ));
                    }
                }
                Err(e) => blockers.push(e),
            }

            file_digests.push(format!(
                "{}:{}",
                path.file_name().and_then(|s| s.to_str()).unwrap_or("f"),
                sha256_file(&path)
            ));

            let fname = path
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("f")
                .to_owned();
            for field_id in &fields {
                for &tsecs in &times {
                    expected_case_ids.insert(format!("{fname}:{field_id}:{tsecs}"));
                }
            }

            for field_id in &fields {
                for &tsecs in &times {
                    let slab_id = format!("{fname}:{field_id}:{tsecs}");
                    let decoded = if format == "netcdf" {
                        #[cfg(feature = "native-netcdf")]
                        {
                            let var = field_id
                                .rsplit_once(':')
                                .map(|(_, v)| v)
                                .unwrap_or(field_id);
                            match trajecta_case::model::time::Timestamp::new(tsecs, 0) {
                                Ok(vt) => decode_netcdf_pair(&path, var, vt),
                                Err(e) => Err(format!("timestamp: {e:?}")),
                            }
                        }
                        #[cfg(not(feature = "native-netcdf"))]
                        {
                            Err("native-netcdf required".into())
                        }
                    } else {
                        #[cfg(feature = "native-eccodes")]
                        {
                            match trajecta_case::model::time::Timestamp::new(tsecs, 0) {
                                Ok(t) => decode_grib_pair(&path, field_id, Some(t)),
                                Err(e) => Err(format!("timestamp: {e:?}")),
                            }
                        }
                        #[cfg(not(feature = "native-eccodes"))]
                        {
                            Err("native-eccodes required".into())
                        }
                    };
                    match decoded {
                        Ok((rf, nf, rm, nm)) => {
                            match make_slab(
                                slab_id.clone(),
                                &family,
                                &variant,
                                field_id,
                                tsecs,
                                &rf,
                                &nf,
                                &rm,
                                &nm,
                            ) {
                                Ok(slab) => slabs.push(slab),
                                Err(e) => blockers.push(e),
                            }
                        }
                        Err(e) => blockers.push(format!("{slab_id}:{e}")),
                    }
                }
            }
        }

        file_digests.sort();
        let matrix_sha = sha256_file(&manifest_path);
        let combo =
            sha256_bytes(format!("matrix={matrix_sha}\n{}", file_digests.join("\n")).as_bytes());
        // Frozen combination identity (manifest + files), not first/last file alone.
        let subject_sha = sha256_bytes(format!("rust-reader+{combo}").as_bytes());
        let reference_sha = sha256_bytes(format!("native-reader+{combo}").as_bytes());

        let expected: Vec<String> = expected_case_ids.into_iter().collect();
        if expected.is_empty() {
            blockers.push("expected_matrix_empty".into());
        }

        let report = adjudicate(
            &report_id,
            &reg,
            ComparisonIdentity {
                dataset_family: family,
                comparison_target: "backend_equivalence".into(),
                comparison_variant: variant,
                subject_sha256: subject_sha,
                reference_sha256: reference_sha,
            },
            &expected,
            &slabs,
            &blockers,
        );

        let body = match report_to_json(&report) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("{e}");
                return ExitCode::FAILURE;
            }
        };
        if let Some(parent) = out.parent() {
            let _ = fs::create_dir_all(parent);
        }
        // Atomic-ish write
        let tmp = out.with_extension("json.part");
        if fs::write(&tmp, &body).is_err() {
            eprintln!("write failed");
            return ExitCode::FAILURE;
        }
        if fs::rename(&tmp, &out).is_err() {
            let _ = fs::copy(&tmp, &out);
            let _ = fs::remove_file(&tmp);
        }
        println!(
            "wrote {} status={:?} slabs={} expected={} pairs={} blockers={}",
            out.display(),
            report.status,
            slabs.len(),
            expected.len(),
            slabs
                .iter()
                .map(|s| s.expected_element_count)
                .sum::<usize>(),
            report.blockers.len()
        );
        match report.status {
            ComparisonStatus::Passed => ExitCode::SUCCESS,
            ComparisonStatus::Failed => ExitCode::from(1),
            ComparisonStatus::Incomplete | ComparisonStatus::Unvalidated => ExitCode::from(2),
        }
    }
}

fn main() -> ExitCode {
    #[cfg(any(feature = "native-netcdf", feature = "native-eccodes"))]
    {
        run::main()
    }
    #[cfg(not(any(feature = "native-netcdf", feature = "native-eccodes")))]
    {
        eprintln!("rebuild with --features native-netcdf and/or native-eccodes");
        ExitCode::FAILURE
    }
}
