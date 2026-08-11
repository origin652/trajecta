//! Real GMTED2010 source-identity and Rasterio cross-check gate for M6-A2.

#![allow(clippy::unwrap_used, clippy::panic)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use trajecta_case::document::DataRootId;
use trajecta_case::lockfile::GeneratorInfo;
use trajecta_met::auxiliary::gmted2010::{
    GMTED2010_MEAN_GRID_FILE, GMTED2010_SOURCE_MANIFEST_FILE,
    GMTED2010_STANDARD_DEVIATION_GRID_FILE, build_global_dataset_lock, open_from_dataset_lock,
};

const FROZEN_FILES: [(&str, u64, &str); 3] = [
    (
        GMTED2010_MEAN_GRID_FILE,
        274_915_129,
        "54070d724ab6aee76944b6fc10a029bd865efbf4c90747088c8f6d9404dabe3e",
    ),
    (
        GMTED2010_STANDARD_DEVIATION_GRID_FILE,
        143_929_399,
        "f2359a29c5790fbd8bf0298470269453a45b329d1312cab88666c92355fadb97",
    ),
    (
        GMTED2010_SOURCE_MANIFEST_FILE,
        8_792,
        "52e81a30ec3cdd44c7202325896bf40f8b6b4a9d0f5f8dd8fd29c4b0d2c97011",
    ),
];

// Values were sampled independently from the original Arc/Info Binary Grids
// with Rasterio 1.4.3, using the same pixel-centred bilinear definition.
const RASTERIO_SAMPLES: [(f64, f64, f64, f64); 9] = [
    (-179.99, 0.0, 0.0, 0.0),
    (179.99, 0.0, 0.0, 0.0),
    (86.925, 27.99, 8_408.266_838_177_557, 135.544_167_972_697),
    (7.65, 46.55, 1_136.743_422_753_239_2, 131.098_243_500_592_28),
    (
        -70.0,
        -32.5,
        4_059.753_833_094_330_3,
        139.546_390_157_704_48,
    ),
    (-122.25, 46.2, 1_204.880_159_316_707, 52.513_634_496_014_22),
    (
        18.4,
        -33.9,
        2.698_690_894_058_479_3,
        2.449_999_681_755_798_5,
    ),
    (135.5, 35.2, 369.784_645_953_500_1, 52.787_470_466_491_975),
    (0.0, 80.0, 0.0, 0.0),
];

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .unwrap()
        .to_path_buf()
}

fn prepared_directory() -> PathBuf {
    std::env::var_os("TRAJECTA_REAL_GMTED2010_DIR").map_or_else(
        || workspace_root().join("target/m6-a2-gmted/prepared"),
        PathBuf::from,
    )
}

fn required() -> bool {
    std::env::var("TRAJECTA_REQUIRE_M6_A2_GMTED")
        .is_ok_and(|value| matches!(value.as_str(), "1" | "true" | "TRUE" | "yes" | "YES"))
}

#[test]
fn real_prepared_pair_matches_frozen_source_identity_and_rasterio_samples() {
    let directory = prepared_directory();
    if !FROZEN_FILES
        .iter()
        .all(|(name, _, _)| directory.join(name).is_file())
    {
        assert!(
            !required(),
            "required GMTED2010 runtime files are missing under {}",
            directory.display()
        );
        eprintln!(
            "skip: GMTED2010 runtime files missing under {}",
            directory.display()
        );
        return;
    }

    let roots = BTreeMap::from([(DataRootId("auxiliary".into()), directory.clone())]);
    let lock = build_global_dataset_lock(
        &roots,
        GeneratorInfo {
            tool: "trajecta-met-m6-a2-test".into(),
            version: env!("CARGO_PKG_VERSION").into(),
        },
    )
    .unwrap();
    for (name, expected_bytes, expected_sha256) in FROZEN_FILES {
        let file = lock
            .files
            .iter()
            .find(|file| file.relative_path == Path::new(name))
            .unwrap();
        assert_eq!(file.size_bytes, expected_bytes, "{name}");
        assert_eq!(file.sha256, expected_sha256, "{name}");
    }

    let dataset = open_from_dataset_lock(&lock, &directory, &roots).unwrap();
    for (longitude, latitude, expected_mean, expected_deviation) in RASTERIO_SAMPLES {
        let mean = dataset.mean_elevation_m(longitude, latitude).unwrap();
        let deviation = dataset
            .elevation_standard_deviation_m(longitude, latitude)
            .unwrap();
        assert!(
            (mean - expected_mean).abs() <= 1.0e-8,
            "mean at ({longitude}, {latitude}): {mean} != {expected_mean}"
        );
        assert!(
            (deviation - expected_deviation).abs() <= 1.0e-8,
            "standard deviation at ({longitude}, {latitude}): {deviation} != {expected_deviation}"
        );
    }
}
