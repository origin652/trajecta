//! Dump hybrid PV coefficients from a GRIB2 probe file to JSON.
use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::ExitCode;

use trajecta_case::document::MeteorologyReaderBackend;
use trajecta_met::io::grib::GribReader;
use trajecta_met::io::reader::MetReader;
use trajecta_met::vertical::VerticalTopology;

fn main() -> ExitCode {
    let path = env::args().nth(1).map(PathBuf::from).unwrap_or_else(|| {
        PathBuf::from("target/test-data/era5-cds-hybrid137-official/era5_hybrid_pv_probe.grib")
    });
    let out = env::args().nth(2).map(PathBuf::from).unwrap_or_else(|| {
        PathBuf::from("target/test-data/era5-cds-hybrid137-official/era5_l137_ab_coefficients.json")
    });
    let reader = GribReader::new(MeteorologyReaderBackend::Rust);
    let index = match reader.build_index(&path) {
        Ok(index) => index,
        Err(error) => {
            eprintln!("index failed: {error:?}");
            return ExitCode::FAILURE;
        }
    };
    let Some(topology) = index.vertical_topology() else {
        eprintln!("vertical topology missing");
        return ExitCode::FAILURE;
    };
    let VerticalTopology::HybridPressure(hybrid) = topology else {
        eprintln!("expected hybrid pressure topology, got {topology:?}");
        return ExitCode::FAILURE;
    };
    let a: Vec<f64> = hybrid.coefficients.a_half_pa.iter().copied().collect();
    let b: Vec<f64> = hybrid.coefficients.b_half.iter().copied().collect();
    let payload = serde_json::json!({
        "source_grib": path.display().to_string(),
        "interface_count": a.len(),
        "full_level_count": a.len().saturating_sub(1),
        "a_half_pa": a,
        "b_half": b,
        "active_full_levels": hybrid.active_full_levels.iter().copied().collect::<Vec<_>>(),
        "note": "Coefficients extracted from official CDS ERA5 GRIB PV metadata; not invented.",
    });
    let body = match serde_json::to_string_pretty(&payload) {
        Ok(body) => body + "\n",
        Err(error) => {
            eprintln!("serialize failed: {error}");
            return ExitCode::FAILURE;
        }
    };
    if let Err(error) = fs::write(&out, body) {
        eprintln!("write failed: {error}");
        return ExitCode::FAILURE;
    }
    println!(
        "wrote {} interfaces={} levels={}",
        out.display(),
        a.len(),
        a.len().saturating_sub(1)
    );
    ExitCode::SUCCESS
}
