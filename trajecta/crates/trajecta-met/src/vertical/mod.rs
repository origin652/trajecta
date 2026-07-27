//! # Contract: native vertical coordinates and local columns
//!
//! Hybrid pressure and fixed pressure levels remain native. A query builds a
//! local curved column after horizontal interpolation, validates monotonicity,
//! and locates a vertical bracket without converting whole files to a legacy
//! intermediate representation.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::derive::height::{
    geopotential_to_geometric_height_m, hydrostatic_full_level_geopotential,
};
use crate::derive::pressure::hybrid_pressure_column;
use crate::derive::thermo::{
    moist_air_density_from_source_humidity_kg_m3, project_specific_humidity_nonnegative,
};
use crate::field::{CanonicalField, FieldKey};
use crate::frame::{ArrayLayout, RawField, RawMetFrame};
use crate::grid::{
    CellId, DomainGeometry, GridBackend, GridPoint, HorizontalWeights, RegularLatLonGrid,
};
use crate::io::inventory::LogicalFrameId;
use crate::science::M3_CONSTANTS;

/// Native vertical-coordinate topology.
#[derive(Clone, Debug, PartialEq)]
pub enum VerticalTopology {
    /// Hybrid pressure using interface A/B coefficients.
    HybridPressure(HybridPressureTopology),
    /// Fixed pressure levels.
    PressureLevels(PressureLevels),
}

/// Vertical staggering of an array.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerticalStagger {
    /// Values at full model layers.
    Full,
    /// Values at layer interfaces.
    Interface,
}

/// Complete hybrid interface coefficients.
#[derive(Clone, Debug, PartialEq)]
pub struct HybridCoefficients {
    /// Interface A coefficients in pascals.
    pub a_half_pa: Arc<[f64]>,
    /// Dimensionless interface B coefficients.
    pub b_half: Arc<[f64]>,
}

/// Complete hybrid coordinate plus the full model layers carried by a frame.
///
/// GRIB files may retain the complete coordinate definition while carrying a
/// contiguous subset of model-level data. Model levels are one-based: full
/// level `n` lies between interface coefficient entries `n - 1` and `n`.
#[derive(Clone, Debug, PartialEq)]
pub struct HybridPressureTopology {
    /// Complete interface coefficients for the native model coordinate.
    pub coefficients: HybridCoefficients,
    /// Strictly increasing, contiguous one-based full levels present in arrays.
    pub active_full_levels: Arc<[u16]>,
}

/// Canonical fixed pressure levels ordered from model top toward the surface.
#[derive(Clone, Debug, PartialEq)]
pub struct PressureLevels {
    /// Strictly increasing, finite, positive pressure values in pascals.
    pub pressure_pa: Arc<[f64]>,
}

impl PressureLevels {
    /// Creates a canonical pressure-level vector.
    pub fn new(pressure_pa: Arc<[f64]>) -> Result<Self, PressureLevelsError> {
        let levels = Self { pressure_pa };
        levels.validate()?;
        Ok(levels)
    }

    /// Validates the canonical top-to-bottom pressure ordering contract.
    pub fn validate(&self) -> Result<(), PressureLevelsError> {
        if self.pressure_pa.is_empty() {
            return Err(PressureLevelsError::Empty);
        }
        if self.pressure_pa.iter().any(|value| !value.is_finite()) {
            return Err(PressureLevelsError::NonFinite);
        }
        if self.pressure_pa.iter().any(|value| *value <= 0.0) {
            return Err(PressureLevelsError::NonPositive);
        }
        if self
            .pressure_pa
            .windows(2)
            .any(|levels| levels[1] <= levels[0])
        {
            return Err(PressureLevelsError::NotStrictlyIncreasing);
        }
        Ok(())
    }
}

/// Invalid canonical pressure-level topology.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PressureLevelsError {
    /// No pressure levels were supplied.
    Empty,
    /// At least one pressure value is NaN or infinite.
    NonFinite,
    /// At least one pressure value is zero or negative.
    NonPositive,
    /// Pressure does not increase strictly from model top toward the surface.
    NotStrictlyIncreasing,
}

impl std::fmt::Display for PressureLevelsError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => formatter.write_str("pressure-level topology is empty"),
            Self::NonFinite => formatter.write_str("pressure-level topology is not finite"),
            Self::NonPositive => formatter.write_str("pressure levels must be positive"),
            Self::NotStrictlyIncreasing => formatter.write_str(
                "pressure levels must be strictly increasing in Pa from model top to surface",
            ),
        }
    }
}

impl std::error::Error for PressureLevelsError {}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use trajecta_case::model::meteorology::DomainId;
    use trajecta_case::model::time::Timestamp;

    use crate::field::{CanonicalField, FieldQuality};
    use crate::frame::{ArrayLayout, FrameMetadata, RawField, RawFieldStore, TemporalSupport};
    use crate::io::inventory::LogicalFrameId;
    use crate::profile::graph::GraphUnit;
    use crate::provenance::{ProvenanceRecord, ProvenanceTable};

    #[test]
    fn pressure_levels_enforce_top_to_surface_order() {
        assert!(PressureLevels::new(Arc::from([50_000.0, 100_000.0])).is_ok());
        assert_eq!(
            PressureLevels::new(Arc::from([100_000.0, 50_000.0])).err(),
            Some(PressureLevelsError::NotStrictlyIncreasing)
        );
        assert_eq!(
            PressureLevels::new(Arc::from([0.0, 50_000.0])).err(),
            Some(PressureLevelsError::NonPositive)
        );
        assert_eq!(
            PressureLevels::new(Arc::from([])).err(),
            Some(PressureLevelsError::Empty)
        );
        assert_eq!(
            PressureLevels::new(Arc::from([50_000.0, f64::NAN])).err(),
            Some(PressureLevelsError::NonFinite)
        );
        assert_eq!(
            PressureLevels::new(Arc::from([50_000.0, 50_000.0])).err(),
            Some(PressureLevelsError::NotStrictlyIncreasing)
        );
    }

    fn sample_column(valid: Arc<[bool]>) -> ColumnGeometry {
        ColumnGeometry::new(
            Arc::from([10_000.0, 50_000.0, 90_000.0]),
            Arc::from([16_000.0, 5_000.0, 1_000.0]),
            Arc::from([220.0, 270.0, 290.0]),
            Arc::from([0.001, 0.004, 0.008]),
            Arc::from([[0.25; 4]; 3]),
            VerticalValidity { valid },
            0.0,
            100_000.0,
            Some(20_000.0),
        )
        .unwrap()
    }

    #[test]
    fn height_and_log_pressure_brackets_do_not_extrapolate() {
        let column = sample_column(Arc::from([true, true, true]));
        let height = column.locate_height_asl_m(3_000.0).unwrap();
        assert_eq!(height.first, 1);
        assert_eq!(height.second, 2);
        assert_eq!(height.second_weight, 0.5);

        let pressure = column
            .locate_pressure_pa((10_000.0_f64 * 50_000.0).sqrt())
            .unwrap();
        assert_eq!(pressure.first, 0);
        assert_eq!(pressure.second, 1);
        assert!((pressure.second_weight - 0.5).abs() < 1.0e-15);

        assert_eq!(
            column.locate_height_asl_m(18_000.0),
            Err(VerticalError::AboveAvailableTop)
        );
        assert_eq!(
            column.locate_height_asl_m(21_000.0),
            Err(VerticalError::AboveModelTop)
        );
        assert_eq!(
            column.locate_height_asl_m(500.0),
            Err(VerticalError::SurfaceLayerRequired)
        );
        assert_eq!(
            column.locate_pressure_pa(95_000.0),
            Err(VerticalError::SurfaceLayerRequired)
        );
    }

    #[test]
    fn invalid_middle_level_does_not_create_a_cross_gap_bracket() {
        let column = sample_column(Arc::from([true, false, true]));
        assert_eq!(
            column.locate_height_asl_m(8_000.0),
            Err(VerticalError::NoValidBracket)
        );
    }

    #[test]
    fn bounds_share_column_terrain_top_and_pressure_anchors() {
        let column = sample_column(Arc::from([true, true, true]));
        let bounds = column.vertical_bounds(2.0).unwrap();
        assert_eq!(bounds.terrain_asl_m(), 0.0);
        assert_eq!(bounds.minimum_transport_asl_m(), 2.0);
        assert_eq!(bounds.available_top_asl_m(), 16_000.0);
        assert_eq!(bounds.physical_model_top_asl_m(), Some(20_000.0));
        assert_eq!(bounds.minimum_pressure_pa(), 10_000.0);
        assert_eq!(bounds.maximum_pressure_pa(), 100_000.0);
    }

    fn metadata_for(vertical: VerticalTopology) -> FrameMetadata {
        let domain = DomainId("test".into());
        FrameMetadata {
            id: LogicalFrameId {
                domain: domain.clone(),
                valid_time: Timestamp::UNIX_EPOCH,
                profile_sha256: "profile".into(),
                content_sha256: "content".into(),
            },
            domain: domain.clone(),
            valid_time: Timestamp::UNIX_EPOCH,
            grid: crate::grid::DomainGeometry {
                domain,
                longitude_origin_degrees: 0.0,
                latitude_origin_degrees: 0.0,
                longitude_spacing_degrees: 1.0,
                latitude_spacing_degrees: 1.0,
                nx: 2,
                ny: 2,
                periodic_longitude: false,
                halo_cells: 0,
            },
            vertical,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn insert_test_field(
        store: &mut RawFieldStore,
        provenance: &mut ProvenanceTable,
        metadata: &FrameMetadata,
        canonical: CanonicalField,
        values: Vec<f64>,
        valid: Vec<bool>,
        unit: &str,
        layout: ArrayLayout,
    ) {
        let key = FieldKey::Canonical(canonical);
        let id = provenance
            .intern(ProvenanceRecord {
                field: key.clone(),
                quality: FieldQuality::Source,
                sources: vec![format!("test:{canonical:?}")],
                transforms: Vec::new(),
                fallback_reason: None,
                profile_sha256: metadata.id.profile_sha256.clone(),
            })
            .unwrap();
        let field = RawField::new(
            Arc::from(values),
            Arc::from(valid),
            GraphUnit::parse(unit).unwrap(),
            layout,
            TemporalSupport::Instantaneous {
                valid_time: metadata.valid_time,
            },
            FieldQuality::Source,
            id,
        )
        .unwrap();
        store.insert(key, field).unwrap();
    }

    fn pressure_test_frame(mask_bottom_northeast: bool) -> RawMetFrame {
        pressure_test_frame_with_humidity(
            mask_bottom_northeast,
            vec![0.001; 4].into_iter().chain(vec![0.005; 4]).collect(),
        )
    }

    fn pressure_test_frame_with_humidity(
        mask_bottom_northeast: bool,
        humidity_values: Vec<f64>,
    ) -> RawMetFrame {
        let metadata = metadata_for(VerticalTopology::PressureLevels(
            PressureLevels::new(Arc::from([50_000.0, 90_000.0])).unwrap(),
        ));
        let mut fields = RawFieldStore::new();
        let mut provenance = ProvenanceTable::new();
        let full_layout = ArrayLayout::Full3D {
            levels: 2,
            ny: 2,
            nx: 2,
        };
        let mut full_valid = vec![true; 8];
        if mask_bottom_northeast {
            full_valid[7] = false;
        }
        insert_test_field(
            &mut fields,
            &mut provenance,
            &metadata,
            CanonicalField::AirTemperature,
            vec![260.0; 4].into_iter().chain(vec![290.0; 4]).collect(),
            full_valid.clone(),
            "K",
            full_layout,
        );
        insert_test_field(
            &mut fields,
            &mut provenance,
            &metadata,
            CanonicalField::SpecificHumidity,
            humidity_values,
            full_valid.clone(),
            "1",
            full_layout,
        );
        insert_test_field(
            &mut fields,
            &mut provenance,
            &metadata,
            CanonicalField::GeopotentialHeight,
            vec![5_500.0; 4]
                .into_iter()
                .chain(vec![1_000.0; 4])
                .collect(),
            full_valid,
            "m",
            full_layout,
        );
        for (field, values, unit) in [
            (CanonicalField::SurfacePressure, vec![100_000.0; 4], "Pa"),
            (CanonicalField::SurfaceGeopotential, vec![0.0; 4], "m2/s2"),
        ] {
            insert_test_field(
                &mut fields,
                &mut provenance,
                &metadata,
                field,
                values,
                vec![true; 4],
                unit,
                ArrayLayout::Horizontal2D { ny: 2, nx: 2 },
            );
        }
        RawMetFrame::publish(metadata, fields, Arc::new(provenance)).unwrap()
    }

    fn hybrid_test_frame(active: Arc<[u16]>) -> RawMetFrame {
        let metadata = metadata_for(VerticalTopology::HybridPressure(HybridPressureTopology {
            coefficients: HybridCoefficients {
                a_half_pa: Arc::from([0.0, 0.0, 0.0]),
                b_half: Arc::from([0.0, 0.5, 1.0]),
            },
            active_full_levels: active,
        }));
        let levels = match &metadata.vertical {
            VerticalTopology::HybridPressure(topology) => topology.active_full_levels.len(),
            VerticalTopology::PressureLevels(_) => 0,
        };
        let mut fields = RawFieldStore::new();
        let mut provenance = ProvenanceTable::new();
        let full_layout = ArrayLayout::Full3D {
            levels,
            ny: 2,
            nx: 2,
        };
        let temperatures = if levels == 2 {
            vec![260.0; 4].into_iter().chain(vec![290.0; 4]).collect()
        } else {
            vec![260.0; levels * 4]
        };
        insert_test_field(
            &mut fields,
            &mut provenance,
            &metadata,
            CanonicalField::AirTemperature,
            temperatures,
            vec![true; levels * 4],
            "K",
            full_layout,
        );
        insert_test_field(
            &mut fields,
            &mut provenance,
            &metadata,
            CanonicalField::SpecificHumidity,
            vec![0.001; levels * 4],
            vec![true; levels * 4],
            "1",
            full_layout,
        );
        for (field, values, unit) in [
            (CanonicalField::SurfacePressure, vec![100_000.0; 4], "Pa"),
            (CanonicalField::SurfaceGeopotential, vec![0.0; 4], "m2/s2"),
        ] {
            insert_test_field(
                &mut fields,
                &mut provenance,
                &metadata,
                field,
                values,
                vec![true; 4],
                unit,
                ArrayLayout::Horizontal2D { ny: 2, nx: 2 },
            );
        }
        RawMetFrame::publish(metadata, fields, Arc::new(provenance)).unwrap()
    }

    fn request_for(frame: &RawMetFrame, longitude: f64, latitude: f64) -> ColumnRequest<'_> {
        let grid = RegularLatLonGrid::new(frame.metadata().grid.clone()).unwrap();
        ColumnRequest {
            frame,
            cell: grid.locate_cell(longitude, latitude).unwrap(),
            longitude_degrees: longitude,
            latitude_degrees: latitude,
        }
    }

    #[test]
    fn pressure_builder_uses_valid_triangle_and_rejects_outside_it() {
        let frame = pressure_test_frame(true);
        let inside = PressureColumnBuilder
            .build(request_for(&frame, 0.25, 0.25))
            .unwrap();
        assert_eq!(inside.validity().valid.as_ref(), &[true, true]);
        assert_eq!(inside.pressure_pa().as_ref(), &[50_000.0, 90_000.0]);
        assert!(inside.height_asl_m()[0] > inside.height_asl_m()[1]);

        let outside = PressureColumnBuilder
            .build(request_for(&frame, 0.75, 0.75))
            .unwrap();
        assert_eq!(outside.validity().valid.as_ref(), &[true, false]);
        assert_eq!(
            outside.locate_pressure_pa(90_000.0),
            Err(VerticalError::SurfaceLayerRequired)
        );
    }

    #[test]
    fn pressure_builder_preserves_negative_source_humidity_and_projects_physical_humidity() {
        let frame = pressure_test_frame_with_humidity(
            false,
            vec![0.001; 4].into_iter().chain(vec![-1.0e-9; 4]).collect(),
        );
        let column = PressureColumnBuilder
            .build(request_for(&frame, 0.5, 0.5))
            .unwrap();

        assert_eq!(column.validity().valid.as_ref(), &[true, true]);
        assert_eq!(column.specific_humidity().as_ref(), &[0.001, -1.0e-9]);
        assert_eq!(column.physical_specific_humidity().as_ref(), &[0.001, 0.0]);
        assert!(column.density_kg_m3().iter().all(|value| *value > 0.0));
    }

    #[test]
    fn pressure_builder_rejects_q_at_or_above_one_without_triangle_fallback() {
        for invalid_humidity in [1.0, 1.1] {
            let mut humidity = vec![0.001; 4]
                .into_iter()
                .chain(vec![0.005; 4])
                .collect::<Vec<_>>();
            humidity[7] = invalid_humidity;
            let frame = pressure_test_frame_with_humidity(false, humidity);

            assert!(matches!(
                PressureColumnBuilder.build(request_for(&frame, 0.25, 0.25)),
                Err(VerticalError::NumericalFailure)
            ));
        }
    }

    #[test]
    fn hybrid_builder_integrates_four_corner_columns_before_interpolation() {
        let frame = hybrid_test_frame(Arc::from([1_u16, 2_u16]));
        let column = HybridColumnBuilder
            .build(request_for(&frame, 0.5, 0.5))
            .unwrap();
        assert_eq!(column.pressure_pa().as_ref(), &[25_000.0, 75_000.0]);
        assert!(column.height_asl_m()[0] > column.height_asl_m()[1]);
        assert!(column.density_kg_m3().iter().all(|value| *value > 0.0));
        assert_eq!(
            column.physical_model_top_asl_m(),
            Some(column.height_asl_m()[0])
        );
    }

    #[test]
    fn floating_hybrid_subset_requires_an_absolute_lower_anchor() {
        let frame = hybrid_test_frame(Arc::from([1_u16]));
        assert_eq!(
            HybridColumnBuilder.build(request_for(&frame, 0.5, 0.5)),
            Err(VerticalError::MissingHydrostaticAnchor)
        );
    }
}

/// Per-level structural validity for one local column.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerticalValidity {
    /// True where the complete interpolation support is physically valid.
    pub valid: Arc<[bool]>,
}

/// Query-ready local vertical column.
#[derive(Clone, Debug, PartialEq)]
pub struct ColumnGeometry {
    pressure_pa: Arc<[f64]>,
    height_asl_m: Arc<[f64]>,
    temperature_k: Arc<[f64]>,
    specific_humidity: Arc<[f64]>,
    physical_specific_humidity: Arc<[f64]>,
    density_kg_m3: Arc<[f64]>,
    level_horizontal_weights: Arc<[[f64; 4]]>,
    validity: VerticalValidity,
    terrain_asl_m: f64,
    surface_pressure_pa: f64,
    physical_model_top_asl_m: Option<f64>,
}

/// Minimal local vertical column used by continuous boundary geometry.
///
/// It preserves the pressure/height ordering and structural validity needed
/// for the exact transport bounds, while intentionally omitting temperature,
/// humidity, density, and horizontal-weight columns.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct BoundaryColumnGeometry {
    pressure_pa: Vec<f64>,
    height_asl_m: Vec<f64>,
    valid: Vec<bool>,
    terrain_asl_m: f64,
    surface_pressure_pa: f64,
    physical_model_top_asl_m: Option<f64>,
}

impl BoundaryColumnGeometry {
    pub(crate) fn new(
        pressure_pa: Vec<f64>,
        height_asl_m: Vec<f64>,
        valid: Vec<bool>,
        terrain_asl_m: f64,
        surface_pressure_pa: f64,
        physical_model_top_asl_m: Option<f64>,
    ) -> Result<Self, VerticalError> {
        let levels = pressure_pa.len();
        if levels == 0 || height_asl_m.len() != levels || valid.len() != levels {
            return Err(VerticalError::InvalidTopology);
        }
        if !terrain_asl_m.is_finite()
            || !surface_pressure_pa.is_finite()
            || surface_pressure_pa <= 0.0
            || physical_model_top_asl_m.is_some_and(|value| !value.is_finite())
            || pressure_pa
                .iter()
                .any(|value| !value.is_finite() || *value <= 0.0)
            || pressure_pa.windows(2).any(|values| values[1] <= values[0])
            || height_asl_m.iter().any(|value| !value.is_finite())
            || height_asl_m.windows(2).any(|values| values[1] >= values[0])
        {
            return Err(VerticalError::NonMonotonicColumn);
        }
        let first_valid = valid
            .iter()
            .position(|value| *value)
            .ok_or(VerticalError::InvalidValidityMask)?;
        for index in 0..levels {
            if valid[index]
                && (height_asl_m[index] <= terrain_asl_m
                    || pressure_pa[index] > surface_pressure_pa)
            {
                return Err(VerticalError::InvalidValidityMask);
            }
        }
        if physical_model_top_asl_m.is_some_and(|top| top < height_asl_m[first_valid]) {
            return Err(VerticalError::InvalidPhysicalAnchor);
        }
        Ok(Self {
            pressure_pa,
            height_asl_m,
            valid,
            terrain_asl_m,
            surface_pressure_pa,
            physical_model_top_asl_m,
        })
    }

    #[must_use]
    pub(crate) fn pressure_pa(&self) -> &[f64] {
        &self.pressure_pa
    }

    #[must_use]
    pub(crate) fn height_asl_m(&self) -> &[f64] {
        &self.height_asl_m
    }

    #[must_use]
    pub(crate) fn validity(&self) -> &[bool] {
        &self.valid
    }

    #[must_use]
    pub(crate) const fn terrain_asl_m(&self) -> f64 {
        self.terrain_asl_m
    }

    #[must_use]
    pub(crate) const fn surface_pressure_pa(&self) -> f64 {
        self.surface_pressure_pa
    }

    #[must_use]
    pub(crate) const fn physical_model_top_asl_m(&self) -> Option<f64> {
        self.physical_model_top_asl_m
    }

    pub(crate) fn first_valid_index(&self) -> Result<usize, VerticalError> {
        self.valid
            .iter()
            .position(|value| *value)
            .ok_or(VerticalError::InvalidValidityMask)
    }

    pub(crate) fn last_valid_index(&self) -> Result<usize, VerticalError> {
        self.valid
            .iter()
            .rposition(|value| *value)
            .ok_or(VerticalError::InvalidValidityMask)
    }

    pub(crate) fn vertical_bounds(
        &self,
        minimum_transport_agl_m: f64,
    ) -> Result<VerticalBounds, VerticalBoundsError> {
        let top = self
            .valid
            .iter()
            .position(|value| *value)
            .ok_or(VerticalBoundsError::TopBelowMinimum)?;
        VerticalBounds::new(
            self.terrain_asl_m,
            minimum_transport_agl_m,
            self.terrain_asl_m + minimum_transport_agl_m,
            self.height_asl_m[top],
            self.physical_model_top_asl_m,
            self.pressure_pa[top],
            self.surface_pressure_pa,
        )
    }

    pub(crate) fn locate_height_asl_m(&self, query_asl_m: f64) -> Result<(), VerticalError> {
        if !query_asl_m.is_finite() {
            return Err(VerticalError::NumericalFailure);
        }
        if query_asl_m <= self.terrain_asl_m {
            return Err(VerticalError::BelowGround);
        }
        let first = self.first_valid_index()?;
        let last = self.last_valid_index()?;
        if self
            .physical_model_top_asl_m
            .is_some_and(|top| query_asl_m > top)
        {
            return Err(VerticalError::AboveModelTop);
        }
        if query_asl_m > self.height_asl_m[first] {
            return Err(VerticalError::AboveAvailableTop);
        }
        if query_asl_m < self.height_asl_m[last] {
            return Err(VerticalError::SurfaceLayerRequired);
        }
        Ok(())
    }

    pub(crate) fn locate_height_agl_m(&self, query_agl_m: f64) -> Result<(), VerticalError> {
        if !query_agl_m.is_finite() {
            return Err(VerticalError::NumericalFailure);
        }
        self.locate_height_asl_m(self.terrain_asl_m + query_agl_m)
    }

    pub(crate) fn locate_pressure_pa(&self, query_pressure_pa: f64) -> Result<(), VerticalError> {
        if !query_pressure_pa.is_finite() || query_pressure_pa <= 0.0 {
            return Err(VerticalError::NumericalFailure);
        }
        if query_pressure_pa > self.surface_pressure_pa {
            return Err(VerticalError::BelowGround);
        }
        let first = self.first_valid_index()?;
        let last = self.last_valid_index()?;
        if query_pressure_pa < self.pressure_pa[first] {
            return Err(VerticalError::AboveAvailableTop);
        }
        if query_pressure_pa > self.pressure_pa[last] {
            return Err(VerticalError::SurfaceLayerRequired);
        }
        Ok(())
    }
}

impl ColumnGeometry {
    /// Validates a top-to-surface local column and its physical anchors.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        pressure_pa: Arc<[f64]>,
        height_asl_m: Arc<[f64]>,
        temperature_k: Arc<[f64]>,
        specific_humidity: Arc<[f64]>,
        level_horizontal_weights: Arc<[[f64; 4]]>,
        validity: VerticalValidity,
        terrain_asl_m: f64,
        surface_pressure_pa: f64,
        physical_model_top_asl_m: Option<f64>,
    ) -> Result<Self, VerticalError> {
        let levels = pressure_pa.len();
        if levels == 0
            || height_asl_m.len() != levels
            || temperature_k.len() != levels
            || specific_humidity.len() != levels
            || level_horizontal_weights.len() != levels
            || validity.valid.len() != levels
        {
            return Err(VerticalError::InvalidTopology);
        }
        if !terrain_asl_m.is_finite()
            || !surface_pressure_pa.is_finite()
            || surface_pressure_pa <= 0.0
            || physical_model_top_asl_m.is_some_and(|value| !value.is_finite())
        {
            return Err(VerticalError::InvalidPhysicalAnchor);
        }
        if pressure_pa
            .iter()
            .any(|value| !value.is_finite() || *value <= 0.0)
            || pressure_pa.windows(2).any(|values| values[1] <= values[0])
            || height_asl_m.iter().any(|value| !value.is_finite())
            || height_asl_m.windows(2).any(|values| values[1] >= values[0])
            || temperature_k
                .iter()
                .any(|value| !value.is_finite() || *value <= 0.0)
            || specific_humidity
                .iter()
                .any(|value| !value.is_finite() || *value >= 1.0)
            || level_horizontal_weights.iter().any(|weights| {
                weights
                    .iter()
                    .any(|value| !value.is_finite() || *value < 0.0)
                    || (weights.iter().sum::<f64>() - 1.0).abs() > 1.0e-12
            })
        {
            return Err(VerticalError::NonMonotonicColumn);
        }
        let physical_specific_humidity = specific_humidity
            .iter()
            .copied()
            .map(project_specific_humidity_nonnegative)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| VerticalError::NonMonotonicColumn)?;
        let density_kg_m3 = pressure_pa
            .iter()
            .zip(temperature_k.iter())
            .zip(specific_humidity.iter())
            .map(|((pressure, temperature), humidity)| {
                moist_air_density_from_source_humidity_kg_m3(*pressure, *temperature, *humidity)
                    .map_err(|_| VerticalError::NumericalFailure)
            })
            .collect::<Result<Vec<_>, _>>()?;
        let mut valid_count = 0_usize;
        let mut available_top = None;
        for index in 0..levels {
            if validity.valid[index] {
                valid_count += 1;
                available_top.get_or_insert(height_asl_m[index]);
                if height_asl_m[index] <= terrain_asl_m || pressure_pa[index] > surface_pressure_pa
                {
                    return Err(VerticalError::InvalidValidityMask);
                }
            }
        }
        if valid_count == 0 {
            return Err(VerticalError::InvalidValidityMask);
        }
        if let (Some(physical_top), Some(data_top)) = (physical_model_top_asl_m, available_top) {
            if physical_top < data_top {
                return Err(VerticalError::InvalidPhysicalAnchor);
            }
        }
        Ok(Self {
            pressure_pa,
            height_asl_m,
            temperature_k,
            specific_humidity,
            physical_specific_humidity: Arc::from(physical_specific_humidity),
            density_kg_m3: Arc::from(density_kg_m3),
            level_horizontal_weights,
            validity,
            terrain_asl_m,
            surface_pressure_pa,
            physical_model_top_asl_m,
        })
    }

    /// Returns full-level pressure ordered strictly from model top to surface.
    #[must_use]
    pub const fn pressure_pa(&self) -> &Arc<[f64]> {
        &self.pressure_pa
    }

    /// Returns full-level geometric height ordered strictly downward.
    #[must_use]
    pub const fn height_asl_m(&self) -> &Arc<[f64]> {
        &self.height_asl_m
    }

    /// Returns air temperature in the same full-level order.
    #[must_use]
    pub const fn temperature_k(&self) -> &Arc<[f64]> {
        &self.temperature_k
    }

    /// Returns specific humidity in the same full-level order.
    #[must_use]
    pub const fn specific_humidity(&self) -> &Arc<[f64]> {
        &self.specific_humidity
    }

    /// Returns non-negative humidity used only by physical consumers.
    #[must_use]
    pub const fn physical_specific_humidity(&self) -> &Arc<[f64]> {
        &self.physical_specific_humidity
    }

    /// Returns moist-air density in the same full-level order.
    #[must_use]
    pub const fn density_kg_m3(&self) -> &Arc<[f64]> {
        &self.density_kg_m3
    }

    /// Returns the exact four-corner weights used for each local full level.
    #[must_use]
    pub const fn level_horizontal_weights(&self) -> &Arc<[[f64; 4]]> {
        &self.level_horizontal_weights
    }

    /// Returns structural validity in the same full-level order.
    #[must_use]
    pub const fn validity(&self) -> &VerticalValidity {
        &self.validity
    }

    /// Returns local geometric terrain height.
    #[must_use]
    pub const fn terrain_asl_m(&self) -> f64 {
        self.terrain_asl_m
    }

    /// Returns local surface pressure.
    #[must_use]
    pub const fn surface_pressure_pa(&self) -> f64 {
        self.surface_pressure_pa
    }

    /// Returns the declared physical model top, if known.
    #[must_use]
    pub const fn physical_model_top_asl_m(&self) -> Option<f64> {
        self.physical_model_top_asl_m
    }

    /// Returns measured resident bytes owned by this immutable column.
    #[must_use]
    pub fn resident_bytes(&self) -> u64 {
        let floating = self
            .pressure_pa
            .len()
            .saturating_mul(6)
            .saturating_mul(std::mem::size_of::<f64>());
        let validity = self
            .validity
            .valid
            .len()
            .saturating_mul(std::mem::size_of::<bool>());
        let weights = self
            .level_horizontal_weights
            .len()
            .saturating_mul(std::mem::size_of::<[f64; 4]>());
        let anchors = std::mem::size_of::<f64>() * 2 + std::mem::size_of::<Option<f64>>();
        u64::try_from(
            floating
                .saturating_add(validity)
                .saturating_add(weights)
                .saturating_add(anchors),
        )
        .unwrap_or(u64::MAX)
    }

    /// Builds bounds using the frozen minimum height of a surface model.
    pub fn vertical_bounds(
        &self,
        minimum_transport_agl_m: f64,
    ) -> Result<VerticalBounds, VerticalBoundsError> {
        let top_index = self
            .validity
            .valid
            .iter()
            .position(|valid| *valid)
            .ok_or(VerticalBoundsError::TopBelowMinimum)?;
        VerticalBounds::new(
            self.terrain_asl_m,
            minimum_transport_agl_m,
            self.terrain_asl_m + minimum_transport_agl_m,
            self.height_asl_m[top_index],
            self.physical_model_top_asl_m,
            self.pressure_pa[top_index],
            self.surface_pressure_pa,
        )
    }

    /// Locates a geometric ASL height without extrapolation.
    pub fn locate_height_asl_m(&self, query_asl_m: f64) -> Result<VerticalBracket, VerticalError> {
        if !query_asl_m.is_finite() {
            return Err(VerticalError::NumericalFailure);
        }
        if query_asl_m <= self.terrain_asl_m {
            return Err(VerticalError::BelowGround);
        }
        let first_valid = self.first_valid_index()?;
        let last_valid = self.last_valid_index()?;
        if self
            .physical_model_top_asl_m
            .is_some_and(|top| query_asl_m > top)
        {
            return Err(VerticalError::AboveModelTop);
        }
        if query_asl_m > self.height_asl_m[first_valid] {
            return Err(VerticalError::AboveAvailableTop);
        }
        if query_asl_m < self.height_asl_m[last_valid] {
            return Err(VerticalError::SurfaceLayerRequired);
        }
        locate_descending_linear(&self.height_asl_m, &self.validity.valid, query_asl_m)
    }

    /// Locates an AGL height after converting with this column's terrain.
    pub fn locate_height_agl_m(&self, query_agl_m: f64) -> Result<VerticalBracket, VerticalError> {
        if !query_agl_m.is_finite() {
            return Err(VerticalError::NumericalFailure);
        }
        self.locate_height_asl_m(self.terrain_asl_m + query_agl_m)
    }

    /// Locates pressure with logarithmic-pressure interpolation weights.
    pub fn locate_pressure_pa(
        &self,
        query_pressure_pa: f64,
    ) -> Result<VerticalBracket, VerticalError> {
        if !query_pressure_pa.is_finite() || query_pressure_pa <= 0.0 {
            return Err(VerticalError::NumericalFailure);
        }
        if query_pressure_pa > self.surface_pressure_pa {
            return Err(VerticalError::BelowGround);
        }
        let first_valid = self.first_valid_index()?;
        let last_valid = self.last_valid_index()?;
        if query_pressure_pa < self.pressure_pa[first_valid] {
            return Err(VerticalError::AboveAvailableTop);
        }
        if query_pressure_pa > self.pressure_pa[last_valid] {
            return Err(VerticalError::SurfaceLayerRequired);
        }
        locate_ascending_log_pressure(&self.pressure_pa, &self.validity.valid, query_pressure_pa)
    }

    fn first_valid_index(&self) -> Result<usize, VerticalError> {
        self.validity
            .valid
            .iter()
            .position(|valid| *valid)
            .ok_or(VerticalError::InvalidVerticalColumn)
    }

    fn last_valid_index(&self) -> Result<usize, VerticalError> {
        self.validity
            .valid
            .iter()
            .rposition(|valid| *valid)
            .ok_or(VerticalError::InvalidVerticalColumn)
    }

    /// Returns the highest locally valid full-level index.
    pub fn available_top_index(&self) -> Result<usize, VerticalError> {
        self.first_valid_index()
    }

    /// Returns the lowest locally valid full-level index.
    pub fn lowest_valid_index(&self) -> Result<usize, VerticalError> {
        self.last_valid_index()
    }
}

/// Local vertical domain shared by bounds probes and ordinary queries.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VerticalBounds {
    terrain_asl_m: f64,
    minimum_transport_agl_m: f64,
    minimum_transport_asl_m: f64,
    available_top_asl_m: f64,
    physical_model_top_asl_m: Option<f64>,
    minimum_pressure_pa: f64,
    maximum_pressure_pa: f64,
}

impl VerticalBounds {
    /// Validates and creates one local vertical domain.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        terrain_asl_m: f64,
        minimum_transport_agl_m: f64,
        minimum_transport_asl_m: f64,
        available_top_asl_m: f64,
        physical_model_top_asl_m: Option<f64>,
        minimum_pressure_pa: f64,
        maximum_pressure_pa: f64,
    ) -> Result<Self, VerticalBoundsError> {
        let required = [
            terrain_asl_m,
            minimum_transport_agl_m,
            minimum_transport_asl_m,
            available_top_asl_m,
            minimum_pressure_pa,
            maximum_pressure_pa,
        ];
        if required.iter().any(|value| !value.is_finite())
            || physical_model_top_asl_m.is_some_and(|value| !value.is_finite())
        {
            return Err(VerticalBoundsError::NonFinite);
        }
        if minimum_transport_agl_m <= 0.0 {
            return Err(VerticalBoundsError::NonPositiveMinimumAgl);
        }
        let expected_minimum_asl = terrain_asl_m + minimum_transport_agl_m;
        let scale = expected_minimum_asl.abs().max(1.0);
        if (minimum_transport_asl_m - expected_minimum_asl).abs() > 1.0e-12 * scale {
            return Err(VerticalBoundsError::InconsistentMinimumHeight);
        }
        if available_top_asl_m < minimum_transport_asl_m {
            return Err(VerticalBoundsError::TopBelowMinimum);
        }
        if physical_model_top_asl_m.is_some_and(|top| top < available_top_asl_m) {
            return Err(VerticalBoundsError::PhysicalTopBelowAvailableTop);
        }
        if minimum_pressure_pa <= 0.0 || maximum_pressure_pa <= 0.0 {
            return Err(VerticalBoundsError::NonPositivePressure);
        }
        if maximum_pressure_pa < minimum_pressure_pa {
            return Err(VerticalBoundsError::PressureOrder);
        }
        Ok(Self {
            terrain_asl_m,
            minimum_transport_agl_m,
            minimum_transport_asl_m,
            available_top_asl_m,
            physical_model_top_asl_m,
            minimum_pressure_pa,
            maximum_pressure_pa,
        })
    }

    /// Returns local geometric terrain height above mean sea level.
    #[must_use]
    pub const fn terrain_asl_m(self) -> f64 {
        self.terrain_asl_m
    }

    /// Returns the lowest AGL height accepted by complete transport.
    #[must_use]
    pub const fn minimum_transport_agl_m(self) -> f64 {
        self.minimum_transport_agl_m
    }

    /// Returns the lowest ASL height accepted by complete transport.
    #[must_use]
    pub const fn minimum_transport_asl_m(self) -> f64 {
        self.minimum_transport_asl_m
    }

    /// Returns the highest geometric height carried by this data subset.
    #[must_use]
    pub const fn available_top_asl_m(self) -> f64 {
        self.available_top_asl_m
    }

    /// Returns the physical model top when the source metadata proves it.
    #[must_use]
    pub const fn physical_model_top_asl_m(self) -> Option<f64> {
        self.physical_model_top_asl_m
    }

    /// Returns the smallest queryable pressure.
    #[must_use]
    pub const fn minimum_pressure_pa(self) -> f64 {
        self.minimum_pressure_pa
    }

    /// Returns the largest queryable pressure, normally local surface pressure.
    #[must_use]
    pub const fn maximum_pressure_pa(self) -> f64 {
        self.maximum_pressure_pa
    }
}

/// Invalid local vertical bounds.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VerticalBoundsError {
    /// At least one numeric bound is NaN or infinite.
    NonFinite,
    /// The complete-transport surface model has no positive minimum height.
    NonPositiveMinimumAgl,
    /// Minimum ASL is not terrain plus minimum AGL.
    InconsistentMinimumHeight,
    /// Available data top lies below the minimum queryable height.
    TopBelowMinimum,
    /// Declared physical model top lies below the available data top.
    PhysicalTopBelowAvailableTop,
    /// A pressure bound is zero or negative.
    NonPositivePressure,
    /// Maximum pressure is smaller than minimum pressure.
    PressureOrder,
}

/// Inputs required to construct one local column.
#[derive(Clone, Copy, Debug)]
pub struct ColumnRequest<'a> {
    /// Immutable source frame.
    pub frame: &'a RawMetFrame,
    /// Horizontal cell containing the query point.
    pub cell: CellId,
    /// Query longitude in degrees east.
    pub longitude_degrees: f64,
    /// Query latitude in degrees north.
    pub latitude_degrees: f64,
}

/// Format-neutral local-column builder.
pub trait ColumnBuilder: Send + Sync {
    /// Builds and validates one query-ready local column.
    fn build(&self, request: ColumnRequest<'_>) -> Result<ColumnGeometry, VerticalError>;
}

/// Immutable four-corner vertical support cached once per frame and cell.
///
/// The stencil contains only cell-corner science. A local column is sampled
/// with the exact point weights during execution, so two points in the same
/// cell can never accidentally share a point-specific column.
#[derive(Clone, Debug, PartialEq)]
pub struct ColumnStencil {
    frame: LogicalFrameId,
    cell: CellId,
    points: [GridPoint; 4],
    kind: ColumnStencilKind,
}

#[derive(Clone, Debug, PartialEq)]
enum ColumnStencilKind {
    Hybrid(HybridColumnStencil),
    Pressure(PressureColumnStencil),
}

#[derive(Clone, Debug, PartialEq)]
struct HybridColumnStencil {
    native_coordinate: Arc<[f64]>,
    pressure_pa: [Vec<f64>; 4],
    height_asl_m: [Vec<f64>; 4],
    temperature_k: [Vec<f64>; 4],
    specific_humidity: [Vec<f64>; 4],
    terrain_asl_m: [f64; 4],
    surface_pressure_pa: [f64; 4],
    carries_physical_top: bool,
}

/// Native coordinate used by the complete kinematic vertical-velocity chain.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeCoordinateKind {
    /// Fixed pressure coordinate in pascals.
    PressurePa,
    /// Dimensionless ECMWF hybrid eta label `A/p_ref + B`.
    HybridEta,
}

#[derive(Clone, Debug, PartialEq)]
struct PressureColumnStencil {
    pressure_pa: Arc<[f64]>,
    height_asl_m: [Vec<f64>; 4],
    temperature_k: [Vec<f64>; 4],
    specific_humidity: [Vec<f64>; 4],
    level_valid: [Vec<bool>; 4],
    terrain_asl_m: [f64; 4],
    terrain_valid: [bool; 4],
    surface_pressure_pa: [f64; 4],
    surface_pressure_valid: [bool; 4],
}

/// One horizontally sampled native full-level surface and its local slopes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NativeLevelGeometry {
    /// Pressure at the query point.
    pub pressure_pa: f64,
    /// Geometric height above mean sea level at the query point.
    pub height_asl_m: f64,
    /// Eastward pressure gradient in pascals per metre.
    pub pressure_eastward_gradient_pa_m: f64,
    /// Northward pressure gradient in pascals per metre.
    pub pressure_northward_gradient_pa_m: f64,
    /// Eastward height gradient in metres per metre.
    pub height_eastward_gradient: f64,
    /// Northward height gradient in metres per metre.
    pub height_northward_gradient: f64,
}

/// Local terrain surface and the plane used by the no-penetration condition.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TerrainGeometry {
    /// Geometric terrain height above mean sea level.
    pub height_asl_m: f64,
    /// Eastward terrain slope in metres per metre.
    pub eastward_gradient: f64,
    /// Northward terrain slope in metres per metre.
    pub northward_gradient: f64,
}

impl ColumnStencil {
    /// Builds one cacheable cell stencil from an arbitrary point in the cell.
    pub fn build(request: ColumnRequest<'_>) -> Result<Self, VerticalError> {
        match &request.frame.metadata().vertical {
            VerticalTopology::HybridPressure(_) => Self::build_hybrid(request),
            VerticalTopology::PressureLevels(_) => Self::build_pressure(request),
        }
    }

    /// Returns the stable horizontal cell identity.
    #[must_use]
    pub const fn cell(&self) -> CellId {
        self.cell
    }

    /// Returns source-corner indices in deterministic storage order.
    #[must_use]
    pub const fn points(&self) -> &[GridPoint; 4] {
        &self.points
    }

    /// Returns the native full-level count represented by this stencil.
    #[must_use]
    pub fn level_count(&self) -> usize {
        match &self.kind {
            ColumnStencilKind::Hybrid(stencil) => stencil.pressure_pa[0].len(),
            ColumnStencilKind::Pressure(stencil) => stencil.pressure_pa.len(),
        }
    }

    /// Returns the native full-level coordinate and its physical kind.
    #[must_use]
    pub fn native_coordinate(&self) -> (NativeCoordinateKind, &[f64]) {
        match &self.kind {
            ColumnStencilKind::Hybrid(stencil) => {
                (NativeCoordinateKind::HybridEta, &stencil.native_coordinate)
            }
            ColumnStencilKind::Pressure(stencil) => {
                (NativeCoordinateKind::PressurePa, &stencil.pressure_pa)
            }
        }
    }

    /// Samples and validates one point-specific local column without source I/O.
    pub fn sample(&self, request: ColumnRequest<'_>) -> Result<ColumnGeometry, VerticalError> {
        if request.frame.metadata().id != self.frame {
            return Err(VerticalError::FrameMismatch);
        }
        let grid = query_grid(request)?;
        verify_cell(
            &grid,
            self.cell,
            request.longitude_degrees,
            request.latitude_degrees,
        )?;
        if request.cell != self.cell {
            return Err(VerticalError::CellMismatch);
        }
        let weights = grid
            .horizontal_weights(request.longitude_degrees, request.latitude_degrees)
            .map_err(|_| VerticalError::InvalidHorizontalSupport)?;
        if weights.points != self.points {
            return Err(VerticalError::CellMismatch);
        }
        match &self.kind {
            ColumnStencilKind::Hybrid(stencil) => stencil.sample(weights),
            ColumnStencilKind::Pressure(stencil) => stencil.sample(weights),
        }
    }

    /// Samples only the local vertical geometry required by boundary policies.
    pub(crate) fn sample_boundary(
        &self,
        request: ColumnRequest<'_>,
    ) -> Result<BoundaryColumnGeometry, VerticalError> {
        let (_, weights) = self.weights_for_request(request)?;
        self.sample_boundary_with_weights(&request.frame.metadata().id, request.cell, weights)
    }

    /// Samples boundary-only geometry from already validated horizontal support.
    ///
    /// Continuous-boundary probing locates each point before pinning stencils.
    /// Reusing that support avoids rebuilding and revalidating the same grid for
    /// the before frame, after frame, and surface scalar.
    pub(crate) fn sample_boundary_with_weights(
        &self,
        frame: &LogicalFrameId,
        cell: CellId,
        weights: HorizontalWeights,
    ) -> Result<BoundaryColumnGeometry, VerticalError> {
        if frame != &self.frame {
            return Err(VerticalError::FrameMismatch);
        }
        if cell != self.cell || weights.points != self.points {
            return Err(VerticalError::CellMismatch);
        }
        match &self.kind {
            ColumnStencilKind::Hybrid(stencil) => stencil.sample_boundary(weights),
            ColumnStencilKind::Pressure(stencil) => stencil.sample_boundary(weights),
        }
    }

    /// Samples one native full-level surface and its exact horizontal slopes.
    pub fn level_geometry(
        &self,
        request: ColumnRequest<'_>,
        level: usize,
    ) -> Result<NativeLevelGeometry, VerticalError> {
        let (grid, weights) = self.weights_for_request(request)?;
        match &self.kind {
            ColumnStencilKind::Hybrid(stencil) => {
                let pressure = corners_at_level(&stencil.pressure_pa, level)?;
                let height = corners_at_level(&stencil.height_asl_m, level)?;
                let pressure_gradient = horizontal_gradient(
                    &pressure,
                    &[true; 4],
                    &weights,
                    grid.geometry(),
                    request.latitude_degrees,
                )?;
                let height_gradient = horizontal_gradient(
                    &height,
                    &[true; 4],
                    &weights,
                    grid.geometry(),
                    request.latitude_degrees,
                )?;
                Ok(NativeLevelGeometry {
                    pressure_pa: bilinear(&pressure, &weights.weights)?,
                    height_asl_m: bilinear(&height, &weights.weights)?,
                    pressure_eastward_gradient_pa_m: pressure_gradient.0,
                    pressure_northward_gradient_pa_m: pressure_gradient.1,
                    height_eastward_gradient: height_gradient.0,
                    height_northward_gradient: height_gradient.1,
                })
            }
            ColumnStencilKind::Pressure(stencil) => {
                if level >= stencil.pressure_pa.len() {
                    return Err(VerticalError::InvalidTopology);
                }
                let valid = std::array::from_fn(|corner| stencil.level_valid[corner][level]);
                let height = corners_at_level(&stencil.height_asl_m, level)?;
                let interpolation_weights = valid_triangle_weights(&weights.weights, &valid)
                    .ok_or(VerticalError::InvalidHorizontalSupport)?;
                let height_gradient = horizontal_gradient(
                    &height,
                    &valid,
                    &weights,
                    grid.geometry(),
                    request.latitude_degrees,
                )?;
                Ok(NativeLevelGeometry {
                    pressure_pa: stencil.pressure_pa[level],
                    height_asl_m: bilinear(&height, &interpolation_weights)?,
                    pressure_eastward_gradient_pa_m: 0.0,
                    pressure_northward_gradient_pa_m: 0.0,
                    height_eastward_gradient: height_gradient.0,
                    height_northward_gradient: height_gradient.1,
                })
            }
        }
    }

    /// Samples terrain and the exact local plane gradient used at the ground.
    pub fn terrain_geometry(
        &self,
        request: ColumnRequest<'_>,
    ) -> Result<TerrainGeometry, VerticalError> {
        let (grid, weights) = self.weights_for_request(request)?;
        let (values, valid) = match &self.kind {
            ColumnStencilKind::Hybrid(stencil) => (stencil.terrain_asl_m, [true; 4]),
            ColumnStencilKind::Pressure(stencil) => (stencil.terrain_asl_m, stencil.terrain_valid),
        };
        let interpolation_weights = valid_triangle_weights(&weights.weights, &valid)
            .ok_or(VerticalError::InvalidHorizontalSupport)?;
        let gradient = horizontal_gradient(
            &values,
            &valid,
            &weights,
            grid.geometry(),
            request.latitude_degrees,
        )?;
        Ok(TerrainGeometry {
            height_asl_m: bilinear(&values, &interpolation_weights)?,
            eastward_gradient: gradient.0,
            northward_gradient: gradient.1,
        })
    }

    /// Returns measured bytes owned by the immutable stencil.
    #[must_use]
    pub fn resident_bytes(&self) -> u64 {
        let bytes = match &self.kind {
            ColumnStencilKind::Hybrid(stencil) => {
                let levels = stencil.pressure_pa[0].len();
                levels
                    .saturating_mul(4)
                    .saturating_mul(4)
                    .saturating_mul(std::mem::size_of::<f64>())
                    .saturating_add(levels.saturating_mul(std::mem::size_of::<f64>()))
                    .saturating_add(std::mem::size_of::<HybridColumnStencil>())
            }
            ColumnStencilKind::Pressure(stencil) => {
                let levels = stencil.pressure_pa.len();
                levels
                    .saturating_mul(4)
                    .saturating_mul(3)
                    .saturating_mul(std::mem::size_of::<f64>())
                    .saturating_add(
                        levels
                            .saturating_mul(4)
                            .saturating_mul(std::mem::size_of::<bool>()),
                    )
                    .saturating_add(levels.saturating_mul(std::mem::size_of::<f64>()))
                    .saturating_add(std::mem::size_of::<PressureColumnStencil>())
            }
        };
        u64::try_from(bytes).unwrap_or(u64::MAX)
    }

    fn build_hybrid(request: ColumnRequest<'_>) -> Result<Self, VerticalError> {
        let VerticalTopology::HybridPressure(topology) = &request.frame.metadata().vertical else {
            return Err(VerticalError::WrongVerticalTopology);
        };
        let grid = query_grid(request)?;
        let weights = grid
            .horizontal_weights(request.longitude_degrees, request.latitude_degrees)
            .map_err(|_| VerticalError::InvalidHorizontalSupport)?;
        verify_cell(
            &grid,
            request.cell,
            request.longitude_degrees,
            request.latitude_degrees,
        )?;
        let temperature = required_field(request.frame, CanonicalField::AirTemperature)?;
        let humidity = required_field(request.frame, CanonicalField::SpecificHumidity)?;
        let surface_pressure = required_field(request.frame, CanonicalField::SurfacePressure)?;
        let surface_geopotential =
            required_field(request.frame, CanonicalField::SurfaceGeopotential)?;
        let active = topology.active_full_levels.as_ref();
        let complete_levels = topology.coefficients.a_half_pa.len().saturating_sub(1);
        let first_level = usize::from(*active.first().ok_or(VerticalError::InvalidTopology)?);
        let last_level = usize::from(*active.last().ok_or(VerticalError::InvalidTopology)?);
        if last_level != complete_levels || first_level == 0 {
            return Err(VerticalError::MissingHydrostaticAnchor);
        }
        let coefficient_start = first_level - 1;
        let a_half = &topology.coefficients.a_half_pa[coefficient_start..=last_level];
        let b_half = &topology.coefficients.b_half[coefficient_start..=last_level];
        let native_half = a_half
            .iter()
            .zip(b_half.iter())
            .map(|(a, b)| a / M3_CONSTANTS.hybrid_reference_pressure_pa + b)
            .collect::<Vec<_>>();
        let native_coordinate = native_half
            .windows(2)
            .map(|values| 0.5 * (values[0] + values[1]))
            .collect::<Vec<_>>();
        if native_coordinate.iter().any(|value| !value.is_finite())
            || native_coordinate
                .windows(2)
                .any(|values| values[1] <= values[0])
        {
            return Err(VerticalError::InvalidTopology);
        }
        let mut pressure_pa = std::array::from_fn(|_| Vec::with_capacity(active.len()));
        let mut height_asl_m = std::array::from_fn(|_| Vec::with_capacity(active.len()));
        let mut temperature_k = std::array::from_fn(|_| Vec::with_capacity(active.len()));
        let mut specific_humidity = std::array::from_fn(|_| Vec::with_capacity(active.len()));
        let mut terrain_asl_m = [0.0; 4];
        let mut surface_pressure_pa = [0.0; 4];
        for (corner, point) in weights.points.iter().copied().enumerate() {
            if !field_valid_2d(surface_pressure, point)?
                || !field_valid_2d(surface_geopotential, point)?
            {
                return Err(VerticalError::InvalidHorizontalSupport);
            }
            let local_surface_pressure = field_value_2d(surface_pressure, point)?;
            let surface_geopotential_value = field_value_2d(surface_geopotential, point)?;
            let pressure = hybrid_pressure_column(a_half, b_half, local_surface_pressure)
                .map_err(|_| VerticalError::InvalidTopology)?;
            for level in 0..active.len() {
                if !field_valid_3d(temperature, level, point)?
                    || !field_valid_3d(humidity, level, point)?
                {
                    return Err(VerticalError::InvalidHorizontalSupport);
                }
                temperature_k[corner].push(field_value_3d(temperature, level, point)?);
                specific_humidity[corner].push(field_value_3d(humidity, level, point)?);
            }
            let physical_humidity = specific_humidity[corner]
                .iter()
                .copied()
                .map(project_specific_humidity_nonnegative)
                .collect::<Result<Vec<_>, _>>()
                .map_err(|_| VerticalError::NumericalFailure)?;
            let geopotential = hydrostatic_full_level_geopotential(
                &pressure.interface_pa,
                &temperature_k[corner],
                &physical_humidity,
                surface_geopotential_value,
            )
            .map_err(|_| VerticalError::NumericalFailure)?;
            pressure_pa[corner] = pressure.full_pa;
            height_asl_m[corner] = geopotential
                .full_level_m2_s2
                .iter()
                .map(|value| {
                    geopotential_to_geometric_height_m(*value)
                        .map_err(|_| VerticalError::NumericalFailure)
                })
                .collect::<Result<Vec<_>, _>>()?;
            terrain_asl_m[corner] = geopotential_to_geometric_height_m(surface_geopotential_value)
                .map_err(|_| VerticalError::NumericalFailure)?;
            surface_pressure_pa[corner] = local_surface_pressure;
        }
        Ok(Self {
            frame: request.frame.metadata().id.clone(),
            cell: request.cell,
            points: weights.points,
            kind: ColumnStencilKind::Hybrid(HybridColumnStencil {
                native_coordinate: Arc::from(native_coordinate),
                pressure_pa,
                height_asl_m,
                temperature_k,
                specific_humidity,
                terrain_asl_m,
                surface_pressure_pa,
                carries_physical_top: first_level == 1,
            }),
        })
    }

    fn weights_for_request(
        &self,
        request: ColumnRequest<'_>,
    ) -> Result<(RegularLatLonGrid, HorizontalWeights), VerticalError> {
        if request.frame.metadata().id != self.frame || request.cell != self.cell {
            return Err(VerticalError::FrameMismatch);
        }
        let grid = query_grid(request)?;
        verify_cell(
            &grid,
            self.cell,
            request.longitude_degrees,
            request.latitude_degrees,
        )?;
        let weights = grid
            .horizontal_weights(request.longitude_degrees, request.latitude_degrees)
            .map_err(|_| VerticalError::InvalidHorizontalSupport)?;
        if weights.points != self.points {
            return Err(VerticalError::CellMismatch);
        }
        Ok((grid, weights))
    }

    fn build_pressure(request: ColumnRequest<'_>) -> Result<Self, VerticalError> {
        let VerticalTopology::PressureLevels(topology) = &request.frame.metadata().vertical else {
            return Err(VerticalError::WrongVerticalTopology);
        };
        let grid = query_grid(request)?;
        let weights = grid
            .horizontal_weights(request.longitude_degrees, request.latitude_degrees)
            .map_err(|_| VerticalError::InvalidHorizontalSupport)?;
        verify_cell(
            &grid,
            request.cell,
            request.longitude_degrees,
            request.latitude_degrees,
        )?;
        let temperature = required_field(request.frame, CanonicalField::AirTemperature)?;
        let humidity = required_field(request.frame, CanonicalField::SpecificHumidity)?;
        let surface_pressure = required_field(request.frame, CanonicalField::SurfacePressure)?;
        let height_field = request
            .frame
            .fields()
            .get(&FieldKey::Canonical(CanonicalField::GeometricHeight))
            .map(|field| (field, HeightSource::Geometric))
            .or_else(|| {
                request
                    .frame
                    .fields()
                    .get(&FieldKey::Canonical(CanonicalField::GeopotentialHeight))
                    .map(|field| (field, HeightSource::GeopotentialHeight))
            })
            .ok_or({
                VerticalError::MissingField(FieldKey::Canonical(CanonicalField::GeometricHeight))
            })?;
        let (terrain_asl_m, terrain_valid) = terrain_corners(request.frame, &weights)?;
        let surface_pressure_pa = corner_values_2d(surface_pressure, &weights.points)?;
        let surface_pressure_valid = corner_valid_2d(surface_pressure, &weights.points)?;
        let levels = topology.pressure_pa.len();
        let mut height_asl_m = std::array::from_fn(|_| vec![0.0; levels]);
        let mut temperature_k = std::array::from_fn(|_| vec![273.15; levels]);
        let mut specific_humidity = std::array::from_fn(|_| vec![0.0; levels]);
        let mut level_valid = std::array::from_fn(|_| vec![false; levels]);
        for (corner, point) in weights.points.iter().copied().enumerate() {
            for level in 0..levels {
                let sources_valid = field_valid_3d(height_field.0, level, point)?
                    && field_valid_3d(temperature, level, point)?
                    && field_valid_3d(humidity, level, point)?
                    && terrain_valid[corner]
                    && surface_pressure_valid[corner];
                if !sources_valid {
                    continue;
                }
                let source_height = field_value_3d(height_field.0, level, point)?;
                let geometric_height = match height_field.1 {
                    HeightSource::Geometric => source_height,
                    HeightSource::GeopotentialHeight => {
                        let geopotential = M3_CONSTANTS.standard_gravity_m_s2 * source_height;
                        geopotential_to_geometric_height_m(geopotential)
                            .map_err(|_| VerticalError::NumericalFailure)?
                    }
                };
                let local_temperature = field_value_3d(temperature, level, point)?;
                let local_humidity = field_value_3d(humidity, level, point)?;
                if !local_humidity.is_finite() || local_humidity >= 1.0 {
                    return Err(VerticalError::NumericalFailure);
                }
                if topology.pressure_pa[level] <= surface_pressure_pa[corner]
                    && geometric_height > terrain_asl_m[corner]
                    && local_temperature > 0.0
                {
                    height_asl_m[corner][level] = geometric_height;
                    temperature_k[corner][level] = local_temperature;
                    specific_humidity[corner][level] = local_humidity;
                    level_valid[corner][level] = true;
                }
            }
        }
        Ok(Self {
            frame: request.frame.metadata().id.clone(),
            cell: request.cell,
            points: weights.points,
            kind: ColumnStencilKind::Pressure(PressureColumnStencil {
                pressure_pa: topology.pressure_pa.clone(),
                height_asl_m,
                temperature_k,
                specific_humidity,
                level_valid,
                terrain_asl_m,
                terrain_valid,
                surface_pressure_pa,
                surface_pressure_valid,
            }),
        })
    }

    #[cfg(test)]
    pub(crate) fn synthetic_for_cache(offset: f64) -> Self {
        let pressure_pa = std::array::from_fn(|corner| vec![10_000.0 + offset + corner as f64]);
        let height_asl_m = std::array::from_fn(|corner| vec![15_000.0 + offset + corner as f64]);
        let temperature_k = std::array::from_fn(|corner| vec![220.0 + corner as f64]);
        let specific_humidity = std::array::from_fn(|corner| vec![0.001 + corner as f64 * 1.0e-5]);
        Self {
            frame: LogicalFrameId {
                domain: trajecta_case::model::meteorology::DomainId("synthetic".into()),
                valid_time: trajecta_case::model::time::Timestamp::UNIX_EPOCH,
                profile_sha256: "synthetic".into(),
                content_sha256: format!("{offset}"),
            },
            cell: CellId(0),
            points: [
                GridPoint { x: 0, y: 0 },
                GridPoint { x: 1, y: 0 },
                GridPoint { x: 0, y: 1 },
                GridPoint { x: 1, y: 1 },
            ],
            kind: ColumnStencilKind::Hybrid(HybridColumnStencil {
                native_coordinate: Arc::from([0.1]),
                pressure_pa,
                height_asl_m,
                temperature_k,
                specific_humidity,
                terrain_asl_m: [offset; 4],
                surface_pressure_pa: [100_000.0; 4],
                carries_physical_top: true,
            }),
        }
    }
}

impl HybridColumnStencil {
    fn sample(&self, weights: HorizontalWeights) -> Result<ColumnGeometry, VerticalError> {
        let terrain_asl_m = bilinear(&self.terrain_asl_m, &weights.weights)?;
        let surface_pressure_pa = bilinear(&self.surface_pressure_pa, &weights.weights)?;
        let levels = self.pressure_pa[0].len();
        let mut pressure_pa = Vec::with_capacity(levels);
        let mut height_asl_m = Vec::with_capacity(levels);
        let mut temperature_k = Vec::with_capacity(levels);
        let mut specific_humidity = Vec::with_capacity(levels);
        for level in 0..levels {
            pressure_pa.push(bilinear(
                &corners_at_level(&self.pressure_pa, level)?,
                &weights.weights,
            )?);
            height_asl_m.push(bilinear(
                &corners_at_level(&self.height_asl_m, level)?,
                &weights.weights,
            )?);
            temperature_k.push(bilinear(
                &corners_at_level(&self.temperature_k, level)?,
                &weights.weights,
            )?);
            specific_humidity.push(bilinear(
                &corners_at_level(&self.specific_humidity, level)?,
                &weights.weights,
            )?);
        }
        let physical_top = self.carries_physical_top.then_some(height_asl_m[0]);
        ColumnGeometry::new(
            Arc::from(pressure_pa),
            Arc::from(height_asl_m),
            Arc::from(temperature_k),
            Arc::from(specific_humidity),
            Arc::from(vec![weights.weights; levels]),
            VerticalValidity {
                valid: Arc::from(vec![true; levels]),
            },
            terrain_asl_m,
            surface_pressure_pa,
            physical_top,
        )
    }

    fn sample_boundary(
        &self,
        weights: HorizontalWeights,
    ) -> Result<BoundaryColumnGeometry, VerticalError> {
        let terrain_asl_m = bilinear(&self.terrain_asl_m, &weights.weights)?;
        let surface_pressure_pa = bilinear(&self.surface_pressure_pa, &weights.weights)?;
        let levels = self.pressure_pa[0].len();
        let mut pressure_pa = Vec::with_capacity(levels);
        let mut height_asl_m = Vec::with_capacity(levels);
        for level in 0..levels {
            pressure_pa.push(bilinear(
                &corners_at_level(&self.pressure_pa, level)?,
                &weights.weights,
            )?);
            height_asl_m.push(bilinear(
                &corners_at_level(&self.height_asl_m, level)?,
                &weights.weights,
            )?);
        }
        let physical_top = self.carries_physical_top.then_some(height_asl_m[0]);
        BoundaryColumnGeometry::new(
            pressure_pa,
            height_asl_m,
            vec![true; levels],
            terrain_asl_m,
            surface_pressure_pa,
            physical_top,
        )
    }
}

impl PressureColumnStencil {
    fn sample(&self, weights: HorizontalWeights) -> Result<ColumnGeometry, VerticalError> {
        let terrain_weights = valid_triangle_weights(&weights.weights, &self.terrain_valid)
            .ok_or(VerticalError::InvalidHorizontalSupport)?;
        let pressure_weights =
            valid_triangle_weights(&weights.weights, &self.surface_pressure_valid)
                .ok_or(VerticalError::InvalidHorizontalSupport)?;
        let terrain_asl_m = bilinear(&self.terrain_asl_m, &terrain_weights)?;
        let surface_pressure_pa = bilinear(&self.surface_pressure_pa, &pressure_weights)?;
        let levels = self.pressure_pa.len();
        let mut height_options = vec![None; levels];
        let mut temperature_options = vec![None; levels];
        let mut humidity_options = vec![None; levels];
        let mut level_weights = vec![[0.25; 4]; levels];
        let mut valid = vec![false; levels];
        for level in 0..levels {
            let corner_valid = std::array::from_fn(|corner| self.level_valid[corner][level]);
            let Some(interpolation_weights) =
                valid_triangle_weights(&weights.weights, &corner_valid)
            else {
                continue;
            };
            let height = bilinear(
                &corners_at_level(&self.height_asl_m, level)?,
                &interpolation_weights,
            )?;
            let temperature = bilinear(
                &corners_at_level(&self.temperature_k, level)?,
                &interpolation_weights,
            )?;
            let humidity = bilinear(
                &corners_at_level(&self.specific_humidity, level)?,
                &interpolation_weights,
            )?;
            if height <= terrain_asl_m || self.pressure_pa[level] > surface_pressure_pa {
                continue;
            }
            height_options[level] = Some(height);
            temperature_options[level] = Some(temperature);
            humidity_options[level] = Some(humidity);
            level_weights[level] = interpolation_weights;
            valid[level] = true;
        }
        let height_asl_m = fill_invalid_descending(&height_options)?;
        let temperature_k = fill_invalid_finite(&temperature_options, 273.15)?;
        let specific_humidity = fill_invalid_finite(&humidity_options, 0.0)?;
        ColumnGeometry::new(
            self.pressure_pa.clone(),
            Arc::from(height_asl_m),
            Arc::from(temperature_k),
            Arc::from(specific_humidity),
            Arc::from(level_weights),
            VerticalValidity {
                valid: Arc::from(valid),
            },
            terrain_asl_m,
            surface_pressure_pa,
            None,
        )
    }

    fn sample_boundary(
        &self,
        weights: HorizontalWeights,
    ) -> Result<BoundaryColumnGeometry, VerticalError> {
        let terrain_weights = valid_triangle_weights(&weights.weights, &self.terrain_valid)
            .ok_or(VerticalError::InvalidHorizontalSupport)?;
        let pressure_weights =
            valid_triangle_weights(&weights.weights, &self.surface_pressure_valid)
                .ok_or(VerticalError::InvalidHorizontalSupport)?;
        let terrain_asl_m = bilinear(&self.terrain_asl_m, &terrain_weights)?;
        let surface_pressure_pa = bilinear(&self.surface_pressure_pa, &pressure_weights)?;
        let levels = self.pressure_pa.len();
        let mut height_options = vec![None; levels];
        let mut valid = vec![false; levels];
        for level in 0..levels {
            let corner_valid = std::array::from_fn(|corner| self.level_valid[corner][level]);
            let Some(interpolation_weights) =
                valid_triangle_weights(&weights.weights, &corner_valid)
            else {
                continue;
            };
            let height = bilinear(
                &corners_at_level(&self.height_asl_m, level)?,
                &interpolation_weights,
            )?;
            if height <= terrain_asl_m || self.pressure_pa[level] > surface_pressure_pa {
                continue;
            }
            height_options[level] = Some(height);
            valid[level] = true;
        }
        BoundaryColumnGeometry::new(
            self.pressure_pa.to_vec(),
            fill_invalid_descending(&height_options)?,
            valid,
            terrain_asl_m,
            surface_pressure_pa,
            None,
        )
    }
}

/// Hybrid-pressure local-column builder.
#[derive(Clone, Copy, Debug, Default)]
pub struct HybridColumnBuilder;

impl ColumnBuilder for HybridColumnBuilder {
    fn build(&self, request: ColumnRequest<'_>) -> Result<ColumnGeometry, VerticalError> {
        ColumnStencil::build(request)?.sample(request)
    }
}

/// Pressure-level local-column builder with structural underground masks.
#[derive(Clone, Copy, Debug, Default)]
pub struct PressureColumnBuilder;

impl ColumnBuilder for PressureColumnBuilder {
    fn build(&self, request: ColumnRequest<'_>) -> Result<ColumnGeometry, VerticalError> {
        ColumnStencil::build(request)?.sample(request)
    }
}

#[derive(Clone, Copy)]
enum HeightSource {
    Geometric,
    GeopotentialHeight,
}

fn query_grid(request: ColumnRequest<'_>) -> Result<RegularLatLonGrid, VerticalError> {
    RegularLatLonGrid::new(request.frame.metadata().grid.clone())
        .map_err(|_| VerticalError::InvalidHorizontalSupport)
}

fn verify_cell(
    grid: &RegularLatLonGrid,
    expected: CellId,
    longitude_degrees: f64,
    latitude_degrees: f64,
) -> Result<(), VerticalError> {
    let actual = grid
        .locate_cell(longitude_degrees, latitude_degrees)
        .map_err(|_| VerticalError::InvalidHorizontalSupport)?;
    if actual != expected {
        return Err(VerticalError::CellMismatch);
    }
    Ok(())
}

fn required_field(frame: &RawMetFrame, field: CanonicalField) -> Result<&RawField, VerticalError> {
    frame
        .fields()
        .get(&FieldKey::Canonical(field))
        .ok_or(VerticalError::MissingField(FieldKey::Canonical(field)))
}

fn field_value_2d(field: &RawField, point: GridPoint) -> Result<f64, VerticalError> {
    let ArrayLayout::Horizontal2D { nx, ny } = field.layout() else {
        return Err(VerticalError::WrongFieldLayout);
    };
    if point.x >= nx || point.y >= ny {
        return Err(VerticalError::WrongFieldLayout);
    }
    field
        .values()
        .get(point.y * nx + point.x)
        .copied()
        .ok_or(VerticalError::WrongFieldLayout)
}

fn field_valid_2d(field: &RawField, point: GridPoint) -> Result<bool, VerticalError> {
    let ArrayLayout::Horizontal2D { nx, ny } = field.layout() else {
        return Err(VerticalError::WrongFieldLayout);
    };
    if point.x >= nx || point.y >= ny {
        return Err(VerticalError::WrongFieldLayout);
    }
    field
        .validity()
        .get(point.y * nx + point.x)
        .ok_or(VerticalError::WrongFieldLayout)
}

fn field_value_3d(field: &RawField, level: usize, point: GridPoint) -> Result<f64, VerticalError> {
    let ArrayLayout::Full3D { levels, nx, ny } = field.layout() else {
        return Err(VerticalError::WrongFieldLayout);
    };
    if level >= levels || point.x >= nx || point.y >= ny {
        return Err(VerticalError::WrongFieldLayout);
    }
    let index = (level * ny + point.y) * nx + point.x;
    field
        .values()
        .get(index)
        .copied()
        .ok_or(VerticalError::WrongFieldLayout)
}

fn field_valid_3d(field: &RawField, level: usize, point: GridPoint) -> Result<bool, VerticalError> {
    let ArrayLayout::Full3D { levels, nx, ny } = field.layout() else {
        return Err(VerticalError::WrongFieldLayout);
    };
    if level >= levels || point.x >= nx || point.y >= ny {
        return Err(VerticalError::WrongFieldLayout);
    }
    let index = (level * ny + point.y) * nx + point.x;
    field
        .validity()
        .get(index)
        .ok_or(VerticalError::WrongFieldLayout)
}

fn corners_at_level(values: &[Vec<f64>], level: usize) -> Result<[f64; 4], VerticalError> {
    if values.len() != 4 {
        return Err(VerticalError::InvalidHorizontalSupport);
    }
    Ok([
        *values[0].get(level).ok_or(VerticalError::InvalidTopology)?,
        *values[1].get(level).ok_or(VerticalError::InvalidTopology)?,
        *values[2].get(level).ok_or(VerticalError::InvalidTopology)?,
        *values[3].get(level).ok_or(VerticalError::InvalidTopology)?,
    ])
}

fn bilinear(values: &[f64; 4], weights: &[f64; 4]) -> Result<f64, VerticalError> {
    let value = values[0].mul_add(
        weights[0],
        values[1].mul_add(
            weights[1],
            values[2].mul_add(weights[2], values[3] * weights[3]),
        ),
    );
    if !value.is_finite() {
        return Err(VerticalError::NumericalFailure);
    }
    Ok(value)
}

fn corner_values_2d(field: &RawField, points: &[GridPoint; 4]) -> Result<[f64; 4], VerticalError> {
    Ok([
        field_value_2d(field, points[0])?,
        field_value_2d(field, points[1])?,
        field_value_2d(field, points[2])?,
        field_value_2d(field, points[3])?,
    ])
}

fn corner_valid_2d(field: &RawField, points: &[GridPoint; 4]) -> Result<[bool; 4], VerticalError> {
    Ok([
        field_valid_2d(field, points[0])?,
        field_valid_2d(field, points[1])?,
        field_valid_2d(field, points[2])?,
        field_valid_2d(field, points[3])?,
    ])
}

fn terrain_corners(
    frame: &RawMetFrame,
    weights: &HorizontalWeights,
) -> Result<([f64; 4], [bool; 4]), VerticalError> {
    if let Some(terrain) = frame
        .fields()
        .get(&FieldKey::Canonical(CanonicalField::GeometricTerrainHeight))
    {
        return Ok((
            corner_values_2d(terrain, &weights.points)?,
            corner_valid_2d(terrain, &weights.points)?,
        ));
    }
    let geopotential = required_field(frame, CanonicalField::SurfaceGeopotential)?;
    let raw = corner_values_2d(geopotential, &weights.points)?;
    let valid = corner_valid_2d(geopotential, &weights.points)?;
    let mut geometric = [0.0; 4];
    for (index, value) in raw.into_iter().enumerate() {
        if valid[index] {
            geometric[index] = geopotential_to_geometric_height_m(value)
                .map_err(|_| VerticalError::NumericalFailure)?;
        }
    }
    Ok((geometric, valid))
}

/// Returns the deterministic bilinear or valid-triangle weights for one cell.
#[must_use]
pub fn valid_triangle_weights(weights: &[f64; 4], valid: &[bool; 4]) -> Option<[f64; 4]> {
    let count = valid.iter().filter(|value| **value).count();
    if count == 4 {
        return Some(*weights);
    }
    if count != 3 {
        return None;
    }
    let fx = weights[1] + weights[3];
    let fy = weights[2] + weights[3];
    let tolerance = 64.0 * f64::EPSILON;
    let missing = valid.iter().position(|value| !value)?;
    match missing {
        0 if fx + fy >= 1.0 - tolerance => [0.0, 1.0 - fy, 1.0 - fx, fx + fy - 1.0],
        1 if fy >= fx - tolerance => [1.0 - fy, 0.0, fy - fx, fx],
        2 if fx >= fy - tolerance => [1.0 - fx, fx - fy, 0.0, fy],
        3 if fx + fy <= 1.0 + tolerance => [1.0 - fx - fy, fx, fy, 0.0],
        _ => return None,
    }
    .into()
}

fn horizontal_gradient(
    values: &[f64; 4],
    valid: &[bool; 4],
    weights: &HorizontalWeights,
    geometry: &DomainGeometry,
    query_latitude_degrees: f64,
) -> Result<(f64, f64), VerticalError> {
    if values.iter().any(|value| !value.is_finite()) {
        return Err(VerticalError::NumericalFailure);
    }
    valid_triangle_weights(&weights.weights, valid)
        .ok_or(VerticalError::InvalidHorizontalSupport)?;
    let fx = weights.weights[1] + weights.weights[3];
    let fy = weights.weights[2] + weights.weights[3];
    let valid_count = valid.iter().filter(|value| **value).count();
    let (fractional_x, fractional_y) = if valid_count == 4 {
        (
            (1.0 - fy) * (values[1] - values[0]) + fy * (values[3] - values[2]),
            (1.0 - fx) * (values[2] - values[0]) + fx * (values[3] - values[1]),
        )
    } else {
        match valid.iter().position(|value| !value) {
            Some(0) => (values[3] - values[2], values[3] - values[1]),
            Some(1) => (values[3] - values[2], values[2] - values[0]),
            Some(2) => (values[1] - values[0], values[3] - values[1]),
            Some(3) => (values[1] - values[0], values[2] - values[0]),
            _ => return Err(VerticalError::InvalidHorizontalSupport),
        }
    };
    let eastward_cell_m = M3_CONSTANTS.earth_radius_m
        * query_latitude_degrees.to_radians().cos()
        * geometry.longitude_spacing_degrees.to_radians();
    let northward_cell_m =
        M3_CONSTANTS.earth_radius_m * geometry.latitude_spacing_degrees.to_radians();
    if !eastward_cell_m.is_finite()
        || !northward_cell_m.is_finite()
        || eastward_cell_m == 0.0
        || northward_cell_m == 0.0
    {
        return Err(VerticalError::NumericalFailure);
    }
    let eastward = fractional_x / eastward_cell_m;
    let northward = fractional_y / northward_cell_m;
    if !eastward.is_finite() || !northward.is_finite() {
        return Err(VerticalError::NumericalFailure);
    }
    Ok((eastward, northward))
}

fn fill_invalid_descending(values: &[Option<f64>]) -> Result<Vec<f64>, VerticalError> {
    let valid_indices = values
        .iter()
        .enumerate()
        .filter_map(|(index, value)| value.map(|height| (index, height)))
        .collect::<Vec<_>>();
    let Some((first_index, first_height)) = valid_indices.first().copied() else {
        return Err(VerticalError::InvalidPressureSupport);
    };
    if valid_indices.windows(2).any(|pair| pair[1].1 >= pair[0].1) {
        return Err(VerticalError::NonMonotonicColumn);
    }
    let mut output = vec![0.0; values.len()];
    for (index, value) in output.iter_mut().enumerate().take(first_index) {
        *value = first_height + (first_index - index) as f64 * 1_000.0;
    }
    for pair in valid_indices.windows(2) {
        let (left_index, left_height) = pair[0];
        let (right_index, right_height) = pair[1];
        output[left_index] = left_height;
        let span = (right_index - left_index) as f64;
        for (index, value) in output
            .iter_mut()
            .enumerate()
            .take(right_index)
            .skip(left_index + 1)
        {
            let weight = (index - left_index) as f64 / span;
            *value = (right_height - left_height).mul_add(weight, left_height);
        }
    }
    let (last_index, last_height) = *valid_indices
        .last()
        .ok_or(VerticalError::InvalidPressureSupport)?;
    output[last_index] = last_height;
    for (index, value) in output.iter_mut().enumerate().skip(last_index + 1) {
        *value = last_height - (index - last_index) as f64 * 1_000.0;
    }
    Ok(output)
}

fn fill_invalid_finite(values: &[Option<f64>], fallback: f64) -> Result<Vec<f64>, VerticalError> {
    if !fallback.is_finite() {
        return Err(VerticalError::NumericalFailure);
    }
    Ok(values
        .iter()
        .map(|value| value.unwrap_or(fallback))
        .collect())
}

/// Located vertical support and deterministic interpolation weight.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VerticalBracket {
    /// First index in native top-to-surface order.
    pub first: usize,
    /// Second index in native top-to-surface order.
    pub second: usize,
    /// Weight assigned to `second`; exact-level brackets use zero.
    pub second_weight: f64,
}

impl VerticalBracket {
    /// Interpolates one finite value column in a fixed arithmetic order.
    pub fn interpolate(self, values: &[f64]) -> Result<f64, VerticalError> {
        let first = values
            .get(self.first)
            .copied()
            .ok_or(VerticalError::InvalidVerticalColumn)?;
        let second = values
            .get(self.second)
            .copied()
            .ok_or(VerticalError::InvalidVerticalColumn)?;
        if !first.is_finite()
            || !second.is_finite()
            || !self.second_weight.is_finite()
            || !(0.0..=1.0).contains(&self.second_weight)
        {
            return Err(VerticalError::NumericalFailure);
        }
        if self.first == self.second {
            return Ok(first);
        }
        let value = (second - first).mul_add(self.second_weight, first);
        if !value.is_finite() {
            return Err(VerticalError::NumericalFailure);
        }
        Ok(value)
    }
}

fn locate_descending_linear(
    coordinates: &[f64],
    valid: &[bool],
    query: f64,
) -> Result<VerticalBracket, VerticalError> {
    if let Some(index) = coordinates
        .iter()
        .enumerate()
        .find_map(|(index, value)| (*value == query && valid[index]).then_some(index))
    {
        return Ok(VerticalBracket {
            first: index,
            second: index,
            second_weight: 0.0,
        });
    }
    for index in 0..coordinates.len().saturating_sub(1) {
        if valid[index]
            && valid[index + 1]
            && coordinates[index] > query
            && query > coordinates[index + 1]
        {
            let weight =
                (coordinates[index] - query) / (coordinates[index] - coordinates[index + 1]);
            return Ok(VerticalBracket {
                first: index,
                second: index + 1,
                second_weight: weight,
            });
        }
    }
    Err(VerticalError::NoValidBracket)
}

fn locate_ascending_log_pressure(
    coordinates: &[f64],
    valid: &[bool],
    query: f64,
) -> Result<VerticalBracket, VerticalError> {
    if let Some(index) = coordinates
        .iter()
        .enumerate()
        .find_map(|(index, value)| (*value == query && valid[index]).then_some(index))
    {
        return Ok(VerticalBracket {
            first: index,
            second: index,
            second_weight: 0.0,
        });
    }
    for index in 0..coordinates.len().saturating_sub(1) {
        if valid[index]
            && valid[index + 1]
            && coordinates[index] < query
            && query < coordinates[index + 1]
        {
            let lower_log = coordinates[index].ln();
            let upper_log = coordinates[index + 1].ln();
            let weight = (query.ln() - lower_log) / (upper_log - lower_log);
            if !weight.is_finite() {
                return Err(VerticalError::NumericalFailure);
            }
            return Ok(VerticalBracket {
                first: index,
                second: index + 1,
                second_weight: weight,
            });
        }
    }
    Err(VerticalError::NoValidBracket)
}

/// Vertical-column construction or lookup failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum VerticalError {
    /// The selected builder does not match the frame's native topology.
    WrongVerticalTopology,
    /// A cached stencil was sampled with a different logical source frame.
    FrameMismatch,
    /// A hybrid subset does not reach the surface and lacks an absolute lower anchor.
    MissingHydrostaticAnchor,
    /// Horizontal cell, weights, masks, or surface anchors cannot support the point.
    InvalidHorizontalSupport,
    /// Caller-provided cell identity disagrees with the geographic point.
    CellMismatch,
    /// A required canonical source field is absent.
    MissingField(FieldKey),
    /// A required source field has an incompatible canonical array layout.
    WrongFieldLayout,
    /// Coefficient or level arrays have invalid lengths.
    InvalidTopology,
    /// Terrain, surface pressure, or physical model top is invalid.
    InvalidPhysicalAnchor,
    /// Structural validity contradicts terrain or surface pressure.
    InvalidValidityMask,
    /// Pressure or height is not monotonic where required.
    NonMonotonicColumn,
    /// Query point is below the physical surface.
    BelowGround,
    /// Query lies between the surface and the lowest valid three-dimensional level.
    SurfaceLayerRequired,
    /// Query lies above the highest level present in this data subset.
    AboveAvailableTop,
    /// Query point is above the model top.
    AboveModelTop,
    /// No structurally valid pressure-level interpolation support exists.
    InvalidPressureSupport,
    /// No adjacent valid pair brackets the requested coordinate.
    NoValidBracket,
    /// Column publication unexpectedly contains no valid levels.
    InvalidVerticalColumn,
    /// Finite interpolation arithmetic failed.
    NumericalFailure,
}
