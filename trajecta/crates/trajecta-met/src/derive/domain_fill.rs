//! # Contract: dry-air mass derivation
//!
//! Builds a deterministic finite-volume snapshot for the complete safe core of
//! one regular latitude/longitude meteorological domain. Pressure-level and
//! native hybrid-pressure columns share the same underground clipping, dry-air
//! mass, boundary-face geometry, and fixed-order compensated summation rules.

use trajecta_case::model::meteorology::DomainId;
use trajecta_case::model::time::{Direction, Timestamp};

use super::{DeriveError, DeriveRequest, DerivedField, FieldDeriver};
use crate::derive::height::{
    geopotential_to_geometric_height_m, hydrostatic_full_level_geopotential,
};
use crate::derive::pressure::hybrid_pressure_column;
use crate::derive::thermo::project_specific_humidity_nonnegative;
use crate::field::{CanonicalField, FieldKey};
use crate::frame::{ArrayLayout, PreparedWindow};
use crate::grid::{DomainGeometry, GridBackend, HorizontalWeights, RegularLatLonGrid};
use crate::science::M3_CONSTANTS;
use crate::surface_layer::minimum_transport_height_agl_m;
use crate::vertical::VerticalTopology;

/// Stable finite-volume dry-air grid derivation identity.
pub const AIR_MASS_GRID_ALGORITHM_ID: &str = "dry_air_finite_volume_grid/v1";
/// Stable hydrostatic boundary-face flux identity.
pub const BOUNDARY_MASS_FLUX_ALGORITHM_ID: &str = "dry_air_boundary_flux/v1";

/// Cardinal side of a finite meteorological domain.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum BoundarySide {
    /// Minimum safe-core longitude.
    West,
    /// Maximum safe-core longitude.
    East,
    /// Minimum physical latitude.
    South,
    /// Maximum physical latitude.
    North,
}

impl BoundarySide {
    const fn packed_tag(self) -> u64 {
        match self {
            Self::West => 0,
            Self::East => 1,
            Self::South => 2,
            Self::North => 3,
        }
    }
}

/// One dry-air control-volume layer at a native horizontal grid point.
#[derive(Clone, Debug, PartialEq)]
pub struct AirMassLayer {
    /// Zero-based active-array level in top-to-bottom order.
    pub level_index: usize,
    /// One-based native full-level identity.
    pub native_full_level: u16,
    /// Effective upper pressure interface in pascals.
    pub upper_pressure_pa: f64,
    /// Effective lower pressure interface in pascals.
    pub lower_pressure_pa: f64,
    /// Upper geometric interface height above mean sea level.
    pub upper_height_asl_m: f64,
    /// Lower geometric interface height above mean sea level.
    pub lower_height_asl_m: f64,
    /// Native full-level geometric centre height above mean sea level.
    pub centre_height_asl_m: f64,
    /// Time-interpolated native-grid Ertel PV in PVU when available.
    pub potential_vorticity_pvu: Option<f64>,
    /// Full-level air temperature in kelvin.
    pub air_temperature_k: f64,
    /// Full-level specific humidity in `[0, 1)`.
    pub specific_humidity: f64,
    /// Full-level eastward wind in metres per second.
    pub eastward_wind_m_s: f64,
    /// Full-level northward wind in metres per second.
    pub northward_wind_m_s: f64,
    /// Dry-air mass represented by this control-volume layer.
    pub dry_air_mass_kg: f64,
}

impl AirMassLayer {
    /// Returns the positive effective layer pressure thickness.
    #[must_use]
    pub fn pressure_thickness_pa(&self) -> f64 {
        self.lower_pressure_pa - self.upper_pressure_pa
    }

    /// Returns the positive geometric layer thickness.
    #[must_use]
    pub fn geometric_thickness_m(&self) -> f64 {
        self.upper_height_asl_m - self.lower_height_asl_m
    }
}

/// One horizontal finite-volume column in the safe core.
#[derive(Clone, Debug, PartialEq)]
pub struct AirMassColumn {
    /// Stable row-major control-volume identity.
    pub cell_id: u64,
    /// Native x index.
    pub x: usize,
    /// Native y index.
    pub y: usize,
    /// Native grid-point longitude in degrees east.
    pub longitude_degrees: f64,
    /// Native grid-point latitude in degrees north.
    pub latitude_degrees: f64,
    /// Western control-volume edge in an unwrapped longitude coordinate.
    pub west_degrees: f64,
    /// Eastern control-volume edge in the same unwrapped coordinate.
    pub east_degrees: f64,
    /// Southern control-volume edge.
    pub south_degrees: f64,
    /// Northern control-volume edge.
    pub north_degrees: f64,
    /// Exact spherical area of the horizontal control volume.
    pub area_m2: f64,
    /// Local surface pressure in pascals.
    pub surface_pressure_pa: f64,
    /// Local terrain height above mean sea level.
    pub terrain_height_asl_m: f64,
    /// Lowest geometric height accepted by complete transport at the column centre.
    pub transport_floor_height_asl_m: f64,
    /// Pressure at the complete-transport lower boundary.
    pub transport_floor_pressure_pa: f64,
    /// Effective above-ground native layers.
    pub layers: Vec<AirMassLayer>,
    /// Fixed-order compensated sum of layer dry-air mass.
    pub dry_air_mass_kg: f64,
}

/// One finite-domain boundary face and native vertical layer.
#[derive(Clone, Debug, PartialEq)]
pub struct BoundaryFaceLayer {
    /// Stable packed face/layer identity guaranteed to fit signed SQLite IDs.
    pub face_id: u64,
    /// Cardinal boundary side.
    pub side: BoundarySide,
    /// Stable along-boundary control-volume ordinal.
    pub segment_index: usize,
    /// Zero-based active-array level.
    pub level_index: usize,
    /// One-based native full-level identity.
    pub native_full_level: u16,
    /// Boundary face centre longitude.
    pub longitude_degrees: f64,
    /// Boundary face centre latitude.
    pub latitude_degrees: f64,
    /// Tangential lower coordinate: latitude for west/east, longitude for south/north.
    pub tangential_lower_degrees: f64,
    /// Tangential upper coordinate in the same convention.
    pub tangential_upper_degrees: f64,
    /// Effective upper pressure interface.
    pub upper_pressure_pa: f64,
    /// Effective lower pressure interface.
    pub lower_pressure_pa: f64,
    /// Upper geometric interface height.
    pub upper_height_asl_m: f64,
    /// Lower geometric interface height.
    pub lower_height_asl_m: f64,
    /// Native full-level Ertel PV in PVU when available.
    pub potential_vorticity_pvu: Option<f64>,
    /// Layer temperature in kelvin.
    pub air_temperature_k: f64,
    /// Layer specific humidity.
    pub specific_humidity: f64,
    /// Physical inward-normal wind for forward integration.
    pub forward_inward_normal_wind_m_s: f64,
    /// Spherical horizontal edge length.
    pub horizontal_edge_length_m: f64,
    /// Geometric face area from the native-height layer thickness.
    pub geometric_face_area_m2: f64,
}

impl BoundaryFaceLayer {
    /// Returns the integration-direction inward-normal velocity.
    #[must_use]
    pub fn inward_normal_wind_m_s(&self, direction: Direction) -> f64 {
        match direction {
            Direction::Forward => self.forward_inward_normal_wind_m_s,
            Direction::Backward => -self.forward_inward_normal_wind_m_s,
        }
    }

    /// Returns positive dry-air inflow rate for one integration direction.
    pub fn inflow_rate_kg_s(&self, direction: Direction) -> Result<f64, AirMassDerivationError> {
        let inward = self.inward_normal_wind_m_s(direction).max(0.0);
        if inward == 0.0 {
            return Ok(0.0);
        }
        let density = dry_air_density_kg_m3(
            0.5 * (self.upper_pressure_pa + self.lower_pressure_pa),
            self.air_temperature_k,
            self.specific_humidity,
        )?;
        let rate = density * inward * self.geometric_face_area_m2;
        if rate.is_finite() && rate >= 0.0 {
            Ok(rate)
        } else {
            Err(AirMassDerivationError::NumericalFailure)
        }
    }
}

/// Complete dry-air finite-volume state at one physical instant.
#[derive(Clone, Debug, PartialEq)]
pub struct AirMassSnapshot {
    /// Selected domain.
    pub domain: DomainId,
    /// Exact snapshot time.
    pub time: Timestamp,
    /// Immutable regular-grid geometry.
    pub grid: DomainGeometry,
    /// Time-interpolated geometric terrain on the complete native horizontal grid.
    pub terrain_height_grid_asl_m: Vec<f64>,
    /// Highest full-level height available to point queries on the complete grid.
    pub available_top_height_grid_asl_m: Vec<f64>,
    /// Time-interpolated raw aerodynamic roughness on the native horizontal grid.
    pub aerodynamic_roughness_length_grid_m: Vec<f64>,
    /// Safe-core columns in stable y/x order.
    pub columns: Vec<AirMassColumn>,
    /// Finite-domain face/layer records in side/segment/level order.
    pub boundary_faces: Vec<BoundaryFaceLayer>,
    /// Fixed-order compensated total dry-air mass.
    pub total_dry_air_mass_kg: f64,
}

/// Point-specific vertical support used when placing domain-fill particles.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AirMassLocalVerticalBounds {
    /// Bilinearly interpolated terrain height.
    pub terrain_height_asl_m: f64,
    /// Roughness-aware lower boundary of complete transport.
    pub transport_floor_height_asl_m: f64,
    /// Highest bilinearly interpolated full level available to point queries.
    pub available_top_height_asl_m: f64,
}

impl AirMassSnapshot {
    fn horizontal_support(
        &self,
        longitude_degrees: f64,
        latitude_degrees: f64,
    ) -> Result<HorizontalWeights, AirMassDerivationError> {
        RegularLatLonGrid::new(self.grid.clone())
            .map_err(|_| AirMassDerivationError::InvalidGeometry)?
            .horizontal_weights(longitude_degrees, latitude_degrees)
            .map_err(|_| AirMassDerivationError::InvalidGeometry)
    }

    fn sample_horizontal_grid_with_support(
        &self,
        values_grid: &[f64],
        support: &HorizontalWeights,
    ) -> Result<f64, AirMassDerivationError> {
        let expected = self
            .grid
            .nx
            .checked_mul(self.grid.ny)
            .ok_or(AirMassDerivationError::IndexOverflow)?;
        if values_grid.len() != expected {
            return Err(AirMassDerivationError::InvalidLayout);
        }
        let mut values = [0.0; 4];
        for (index, point) in support.points.into_iter().enumerate() {
            let flat = point
                .y
                .checked_mul(self.grid.nx)
                .and_then(|row| row.checked_add(point.x))
                .ok_or(AirMassDerivationError::IndexOverflow)?;
            values[index] = *values_grid
                .get(flat)
                .ok_or(AirMassDerivationError::InvalidLayout)?;
        }
        if values.iter().any(|value| !value.is_finite()) {
            return Err(AirMassDerivationError::InvalidTerrain);
        }
        let value = values[0].mul_add(
            support.weights[0],
            values[1].mul_add(
                support.weights[1],
                values[2].mul_add(support.weights[2], values[3] * support.weights[3]),
            ),
        );
        value
            .is_finite()
            .then_some(value)
            .ok_or(AirMassDerivationError::NumericalFailure)
    }

    fn sample_horizontal_grid(
        &self,
        values_grid: &[f64],
        longitude_degrees: f64,
        latitude_degrees: f64,
    ) -> Result<f64, AirMassDerivationError> {
        let support = self.horizontal_support(longitude_degrees, latitude_degrees)?;
        self.sample_horizontal_grid_with_support(values_grid, &support)
    }

    /// Returns all local vertical placement bounds using one interpolation support.
    pub fn local_vertical_bounds_at(
        &self,
        longitude_degrees: f64,
        latitude_degrees: f64,
    ) -> Result<AirMassLocalVerticalBounds, AirMassDerivationError> {
        let support = self.horizontal_support(longitude_degrees, latitude_degrees)?;
        let terrain_height_asl_m =
            self.sample_horizontal_grid_with_support(&self.terrain_height_grid_asl_m, &support)?;
        let roughness_length_m = self.sample_horizontal_grid_with_support(
            &self.aerodynamic_roughness_length_grid_m,
            &support,
        )?;
        let available_top_height_asl_m = self
            .sample_horizontal_grid_with_support(&self.available_top_height_grid_asl_m, &support)?;
        let minimum_agl = minimum_transport_height_agl_m(roughness_length_m)
            .map_err(|_| AirMassDerivationError::InvalidPhysicalState)?;
        let transport_floor_height_asl_m = terrain_height_asl_m + minimum_agl;
        if !transport_floor_height_asl_m.is_finite() {
            return Err(AirMassDerivationError::NumericalFailure);
        }
        Ok(AirMassLocalVerticalBounds {
            terrain_height_asl_m,
            transport_floor_height_asl_m,
            available_top_height_asl_m,
        })
    }

    /// Bilinearly samples the same local terrain surface used by point queries.
    pub fn terrain_height_asl_m_at(
        &self,
        longitude_degrees: f64,
        latitude_degrees: f64,
    ) -> Result<f64, AirMassDerivationError> {
        self.sample_horizontal_grid(
            &self.terrain_height_grid_asl_m,
            longitude_degrees,
            latitude_degrees,
        )
    }

    /// Bilinearly samples the local highest full level accepted by point queries.
    pub fn available_top_height_asl_m_at(
        &self,
        longitude_degrees: f64,
        latitude_degrees: f64,
    ) -> Result<f64, AirMassDerivationError> {
        self.sample_horizontal_grid(
            &self.available_top_height_grid_asl_m,
            longitude_degrees,
            latitude_degrees,
        )
    }

    /// Returns the local lower boundary of complete transport.
    pub fn transport_floor_height_asl_m_at(
        &self,
        longitude_degrees: f64,
        latitude_degrees: f64,
    ) -> Result<f64, AirMassDerivationError> {
        let terrain = self.terrain_height_asl_m_at(longitude_degrees, latitude_degrees)?;
        let roughness = self.sample_horizontal_grid(
            &self.aerodynamic_roughness_length_grid_m,
            longitude_degrees,
            latitude_degrees,
        )?;
        let minimum_agl = minimum_transport_height_agl_m(roughness)
            .map_err(|_| AirMassDerivationError::InvalidPhysicalState)?;
        let floor = terrain + minimum_agl;
        floor
            .is_finite()
            .then_some(floor)
            .ok_or(AirMassDerivationError::NumericalFailure)
    }
}

/// Dry-air grid and column mass deriver.
#[derive(Clone, Copy, Debug, Default)]
pub struct AirMassDeriver;

impl AirMassDeriver {
    /// Derives a complete time-interpolated domain-fill snapshot without I/O.
    pub fn derive_window(
        &self,
        window: &PreparedWindow,
    ) -> Result<AirMassSnapshot, AirMassDerivationError> {
        let before = window.frames.before.as_ref();
        let after = window.frames.after.as_ref();
        if before.metadata().domain != after.metadata().domain
            || before.metadata().grid != after.metadata().grid
            || before.metadata().vertical != after.metadata().vertical
        {
            return Err(AirMassDerivationError::IncompatibleFrames);
        }

        let grid = before.metadata().grid.clone();
        grid.validate()
            .map_err(|_| AirMassDerivationError::InvalidGeometry)?;
        let fields = InterpolatedFields::from_window(window)?;
        let safe = SafeCore::new(&grid)?;
        let level_count = fields.level_count;
        let native_levels = native_full_levels(&before.metadata().vertical, level_count)?;
        let pressure_interfaces = pressure_interfaces_by_point(
            &before.metadata().vertical,
            &fields.surface_pressure_pa,
            grid.nx,
            grid.ny,
        )?;
        let horizontal_point_count = grid
            .nx
            .checked_mul(grid.ny)
            .ok_or(AirMassDerivationError::IndexOverflow)?;
        let mut available_top_height_grid_asl_m = vec![None; horizontal_point_count];

        let mut columns = Vec::with_capacity(safe.x_indices.len() * safe.y_indices.len());
        for &y in &safe.y_indices {
            for &x in &safe.x_indices {
                let horizontal = horizontal_index(x, y, grid.nx)?;
                let west_degrees = safe.x_lower(x)?;
                let east_degrees = safe.x_upper(x)?;
                let (south_degrees, north_degrees) = safe.y_bounds(y)?;
                let area_m2 = spherical_cell_area_m2(
                    west_degrees,
                    east_degrees,
                    south_degrees,
                    north_degrees,
                )?;
                let surface_pressure_pa = fields.surface_pressure_pa[horizontal];
                let point_interfaces = &pressure_interfaces[horizontal];
                let humidity_column = field_column(
                    &fields.specific_humidity,
                    horizontal,
                    level_count,
                    grid.nx,
                    grid.ny,
                )?;
                let temperature_column = field_column(
                    &fields.air_temperature_k,
                    horizontal,
                    level_count,
                    grid.nx,
                    grid.ny,
                )?;
                for level in 0..level_count {
                    validate_field_value(&humidity_column, level, PhysicalField::Humidity)?;
                    validate_field_value(&temperature_column, level, PhysicalField::Temperature)?;
                }
                let (terrain_height_asl_m, centre_heights) = height_column_at_horizontal(
                    &before.metadata().vertical,
                    &fields,
                    point_interfaces,
                    horizontal,
                    level_count,
                    &grid,
                    (&temperature_column, &humidity_column),
                )?;
                validate_height_column(&centre_heights, terrain_height_asl_m)?;
                if available_top_height_grid_asl_m[horizontal].is_none() {
                    available_top_height_grid_asl_m[horizontal] = centre_heights.first().copied();
                }
                let height_interfaces = height_interfaces(&centre_heights, terrain_height_asl_m)?;
                let full_pressure_pa = match &before.metadata().vertical {
                    VerticalTopology::PressureLevels(topology) => topology.pressure_pa.to_vec(),
                    VerticalTopology::HybridPressure(_) => point_interfaces
                        .windows(2)
                        .map(|interfaces| 0.5 * (interfaces[0] + interfaces[1]))
                        .collect(),
                };
                let roughness_length_m = fields.aerodynamic_roughness_length_m[horizontal];
                let minimum_transport_agl_m = minimum_transport_height_agl_m(roughness_length_m)
                    .map_err(|_| AirMassDerivationError::InvalidPhysicalState)?;
                let transport_floor_height_asl_m = terrain_height_asl_m + minimum_transport_agl_m;
                let last_above_ground_level = (0..level_count)
                    .rfind(|&level| {
                        full_pressure_pa[level] <= surface_pressure_pa
                            && centre_heights[level] > transport_floor_height_asl_m
                    })
                    .ok_or(AirMassDerivationError::NoAboveGroundLayers)?;
                let lowest_height_agl_m =
                    centre_heights[last_above_ground_level] - terrain_height_asl_m;
                if !lowest_height_agl_m.is_finite()
                    || lowest_height_agl_m <= minimum_transport_agl_m
                {
                    return Err(AirMassDerivationError::NoAboveGroundLayers);
                }
                let floor_fraction = minimum_transport_agl_m / lowest_height_agl_m;
                let transport_floor_pressure_pa = transport_floor_pressure(
                    surface_pressure_pa,
                    full_pressure_pa[last_above_ground_level],
                    floor_fraction,
                )?;
                if !transport_floor_height_asl_m.is_finite()
                    || !transport_floor_pressure_pa.is_finite()
                    || transport_floor_pressure_pa <= point_interfaces[last_above_ground_level]
                    || transport_floor_pressure_pa > surface_pressure_pa
                {
                    return Err(AirMassDerivationError::InvalidPhysicalState);
                }
                let mut layers = Vec::with_capacity(last_above_ground_level + 1);
                for level_index in 0..=last_above_ground_level {
                    let upper_pressure_pa = point_interfaces[level_index];
                    let lower_pressure_pa = if level_index == last_above_ground_level {
                        transport_floor_pressure_pa
                    } else {
                        point_interfaces[level_index + 1]
                    };
                    if lower_pressure_pa <= upper_pressure_pa {
                        continue;
                    }
                    let index = full_index(level_index, horizontal, grid.nx, grid.ny)?;
                    validate_field_value(&fields.eastward_wind_m_s, index, PhysicalField::Wind)?;
                    validate_field_value(&fields.northward_wind_m_s, index, PhysicalField::Wind)?;
                    let q = humidity_column[level_index];
                    let lower_height_asl_m = if level_index == last_above_ground_level {
                        transport_floor_height_asl_m
                    } else {
                        height_interfaces[level_index + 1]
                    };
                    let upper_height_asl_m = height_interfaces[level_index];
                    if !upper_height_asl_m.is_finite() || !lower_height_asl_m.is_finite() {
                        return Err(AirMassDerivationError::InvalidHeightColumn);
                    }
                    if upper_height_asl_m <= lower_height_asl_m {
                        return Err(AirMassDerivationError::InvalidHeightColumn);
                    }
                    let dry_air_mass_kg = area_m2 / M3_CONSTANTS.standard_gravity_m_s2
                        * (1.0 - q)
                        * (lower_pressure_pa - upper_pressure_pa);
                    if !dry_air_mass_kg.is_finite() || dry_air_mass_kg <= 0.0 {
                        return Err(AirMassDerivationError::NumericalFailure);
                    }
                    layers.push(AirMassLayer {
                        level_index,
                        native_full_level: native_levels[level_index],
                        upper_pressure_pa,
                        lower_pressure_pa,
                        upper_height_asl_m,
                        lower_height_asl_m,
                        centre_height_asl_m: centre_heights[level_index],
                        potential_vorticity_pvu: fields
                            .potential_vorticity_pvu
                            .as_ref()
                            .and_then(|field| field.valid[index].then_some(field.values[index])),
                        air_temperature_k: temperature_column[level_index],
                        specific_humidity: q,
                        eastward_wind_m_s: fields.eastward_wind_m_s[index],
                        northward_wind_m_s: fields.northward_wind_m_s[index],
                        dry_air_mass_kg,
                    });
                }
                if layers.is_empty() {
                    return Err(AirMassDerivationError::NoAboveGroundLayers);
                }
                let dry_air_mass_kg =
                    neumaier_sum(layers.iter().map(|layer| layer.dry_air_mass_kg))?;
                let cell_id =
                    u64::try_from(horizontal).map_err(|_| AirMassDerivationError::IndexOverflow)?;
                columns.push(AirMassColumn {
                    cell_id,
                    x,
                    y,
                    longitude_degrees: grid.longitude_origin_degrees
                        + grid.longitude_spacing_degrees * x as f64,
                    latitude_degrees: grid.latitude_origin_degrees
                        + grid.latitude_spacing_degrees * y as f64,
                    west_degrees,
                    east_degrees,
                    south_degrees,
                    north_degrees,
                    area_m2,
                    surface_pressure_pa,
                    terrain_height_asl_m,
                    transport_floor_height_asl_m,
                    transport_floor_pressure_pa,
                    layers,
                    dry_air_mass_kg,
                });
            }
        }
        for (horizontal, available_top) in available_top_height_grid_asl_m.iter_mut().enumerate() {
            if available_top.is_some() {
                continue;
            }
            let point_interfaces = pressure_interfaces
                .get(horizontal)
                .ok_or(AirMassDerivationError::InvalidLayout)?;
            let temperature_column = field_column(
                &fields.air_temperature_k,
                horizontal,
                level_count,
                grid.nx,
                grid.ny,
            )?;
            let humidity_column = field_column(
                &fields.specific_humidity,
                horizontal,
                level_count,
                grid.nx,
                grid.ny,
            )?;
            let (terrain_height_asl_m, centre_heights) = height_column_at_horizontal(
                &before.metadata().vertical,
                &fields,
                point_interfaces,
                horizontal,
                level_count,
                &grid,
                (&temperature_column, &humidity_column),
            )?;
            validate_height_column(&centre_heights, terrain_height_asl_m)?;
            *available_top = centre_heights.first().copied();
        }
        let available_top_height_grid_asl_m = available_top_height_grid_asl_m
            .into_iter()
            .collect::<Option<Vec<_>>>()
            .ok_or(AirMassDerivationError::InvalidHeightColumn)?;
        let total_dry_air_mass_kg =
            neumaier_sum(columns.iter().map(|column| column.dry_air_mass_kg))?;
        let boundary_faces = if grid.periodic_longitude {
            Vec::new()
        } else {
            derive_boundary_faces(&grid, &safe, &columns)?
        };
        Ok(AirMassSnapshot {
            domain: before.metadata().domain.clone(),
            time: window.query_time,
            grid,
            terrain_height_grid_asl_m: fields.terrain_height_asl_m,
            available_top_height_grid_asl_m,
            aerodynamic_roughness_length_grid_m: fields.aerodynamic_roughness_length_m,
            columns,
            boundary_faces,
            total_dry_air_mass_kg,
        })
    }
}

fn height_column_at_horizontal(
    topology: &VerticalTopology,
    fields: &InterpolatedFields,
    point_interfaces: &[f64],
    horizontal: usize,
    level_count: usize,
    grid: &DomainGeometry,
    thermodynamic_columns: (&[f64], &[f64]),
) -> Result<(f64, Vec<f64>), AirMassDerivationError> {
    match topology {
        VerticalTopology::PressureLevels(_) => {
            let heights = fields
                .geometric_height_asl_m
                .as_ref()
                .ok_or(AirMassDerivationError::InvalidLayout)?;
            Ok((
                *fields
                    .terrain_height_asl_m
                    .get(horizontal)
                    .ok_or(AirMassDerivationError::InvalidLayout)?,
                field_column(heights, horizontal, level_count, grid.nx, grid.ny)?,
            ))
        }
        VerticalTopology::HybridPressure(_) => {
            let surface_geopotential = fields
                .surface_geopotential_m2_s2
                .as_ref()
                .and_then(|values| values.get(horizontal))
                .copied()
                .ok_or(AirMassDerivationError::InvalidLayout)?;
            let hydrostatic = hydrostatic_full_level_geopotential(
                point_interfaces,
                thermodynamic_columns.0,
                thermodynamic_columns.1,
                surface_geopotential,
            )
            .map_err(|_| AirMassDerivationError::InvalidHeightColumn)?;
            let heights = hydrostatic
                .full_level_m2_s2
                .into_iter()
                .map(|value| {
                    geopotential_to_geometric_height_m(value)
                        .map_err(|_| AirMassDerivationError::InvalidHeightColumn)
                })
                .collect::<Result<Vec<_>, _>>()?;
            let terrain = geopotential_to_geometric_height_m(surface_geopotential)
                .map_err(|_| AirMassDerivationError::InvalidTerrain)?;
            Ok((terrain, heights))
        }
    }
}

impl FieldDeriver for AirMassDeriver {
    fn derive(&self, _request: DeriveRequest<'_>) -> Result<Vec<DerivedField>, DeriveError> {
        // Domain-fill control volumes are not point-sampled canonical fields;
        // callers must use `derive_window` so time support and topology remain explicit.
        Err(DeriveError::InvalidPhysicalState(
            "air-mass control volumes require AirMassDeriver::derive_window".into(),
        ))
    }
}

#[derive(Clone, Debug)]
struct InterpolatedFields {
    level_count: usize,
    surface_pressure_pa: Vec<f64>,
    terrain_height_asl_m: Vec<f64>,
    aerodynamic_roughness_length_m: Vec<f64>,
    geometric_height_asl_m: Option<Vec<f64>>,
    surface_geopotential_m2_s2: Option<Vec<f64>>,
    specific_humidity: Vec<f64>,
    air_temperature_k: Vec<f64>,
    eastward_wind_m_s: Vec<f64>,
    northward_wind_m_s: Vec<f64>,
    potential_vorticity_pvu: Option<InterpolatedField>,
}

impl InterpolatedFields {
    fn from_window(window: &PreparedWindow) -> Result<Self, AirMassDerivationError> {
        let grid = &window.frames.before.metadata().grid;
        let horizontal = grid
            .nx
            .checked_mul(grid.ny)
            .ok_or(AirMassDerivationError::IndexOverflow)?;
        let q = interpolate_specific_humidity(window)?;
        let level_count = match q.layout {
            ArrayLayout::Full3D { levels, ny, nx } if ny == grid.ny && nx == grid.nx => levels,
            _ => return Err(AirMassDerivationError::InvalidLayout),
        };
        let expected_full = level_count
            .checked_mul(horizontal)
            .ok_or(AirMassDerivationError::IndexOverflow)?;
        let surface = interpolate_required_field(window, CanonicalField::SurfacePressure)?;
        let terrain = interpolate_geometric_terrain_height(window)?;
        let roughness =
            interpolate_required_field(window, CanonicalField::AerodynamicRoughnessLength)?;
        let (height, surface_geopotential) = match &window.frames.before.metadata().vertical {
            VerticalTopology::PressureLevels(_) => {
                (Some(interpolate_pressure_geometric_height(window)?), None)
            }
            VerticalTopology::HybridPressure(_) => (
                None,
                Some(interpolate_required_field(
                    window,
                    CanonicalField::SurfaceGeopotential,
                )?),
            ),
        };
        let temperature = interpolate_required_field(window, CanonicalField::AirTemperature)?;
        let eastward = interpolate_required_field(window, CanonicalField::EastwardWind)?;
        let northward = interpolate_required_field(window, CanonicalField::NorthwardWind)?;
        let potential_vorticity =
            interpolate_optional_field(window, CanonicalField::PotentialVorticity)?;
        let mut horizontal_fields = vec![&surface, &terrain, &roughness];
        if let Some(field) = &surface_geopotential {
            horizontal_fields.push(field);
        }
        for field in horizontal_fields {
            if field.layout
                != (ArrayLayout::Horizontal2D {
                    ny: grid.ny,
                    nx: grid.nx,
                })
                || field.values.len() != horizontal
            {
                return Err(AirMassDerivationError::InvalidLayout);
            }
        }
        let mut full_fields = vec![&q, &temperature, &eastward, &northward];
        if let Some(field) = &height {
            full_fields.push(field);
        }
        for field in full_fields {
            if field.layout
                != (ArrayLayout::Full3D {
                    levels: level_count,
                    ny: grid.ny,
                    nx: grid.nx,
                })
                || field.values.len() != expected_full
            {
                return Err(AirMassDerivationError::InvalidLayout);
            }
        }
        if potential_vorticity.as_ref().is_some_and(|field| {
            field.layout
                != (ArrayLayout::Full3D {
                    levels: level_count,
                    ny: grid.ny,
                    nx: grid.nx,
                })
                || field.values.len() != expected_full
        }) {
            return Err(AirMassDerivationError::InvalidLayout);
        }
        for (index, value) in surface.values.iter().copied().enumerate() {
            if !surface.valid[index] || !value.is_finite() || value <= 0.0 {
                return Err(AirMassDerivationError::InvalidSurfacePressure);
            }
        }
        for (index, value) in terrain.values.iter().copied().enumerate() {
            if !terrain.valid[index] || !value.is_finite() {
                return Err(AirMassDerivationError::InvalidTerrain);
            }
        }
        for (index, value) in roughness.values.iter().copied().enumerate() {
            if !roughness.valid[index] || minimum_transport_height_agl_m(value).is_err() {
                return Err(AirMassDerivationError::InvalidPhysicalState);
            }
        }
        let mut required_valid = vec![&q, &temperature, &eastward, &northward];
        if let Some(field) = &height {
            required_valid.push(field);
        }
        for field in required_valid {
            if field.valid.iter().any(|valid| !valid) {
                return Err(AirMassDerivationError::InvalidFieldMask);
            }
        }
        if surface_geopotential
            .as_ref()
            .is_some_and(|field| field.valid.iter().any(|valid| !valid))
        {
            return Err(AirMassDerivationError::InvalidFieldMask);
        }
        Ok(Self {
            level_count,
            surface_pressure_pa: surface.values,
            terrain_height_asl_m: terrain.values,
            aerodynamic_roughness_length_m: roughness.values,
            geometric_height_asl_m: height.map(|field| field.values),
            surface_geopotential_m2_s2: surface_geopotential.map(|field| field.values),
            specific_humidity: q.values,
            air_temperature_k: temperature.values,
            eastward_wind_m_s: eastward.values,
            northward_wind_m_s: northward.values,
            potential_vorticity_pvu: potential_vorticity,
        })
    }
}

#[derive(Clone, Debug)]
struct InterpolatedField {
    values: Vec<f64>,
    valid: Vec<bool>,
    layout: ArrayLayout,
}

fn interpolate_required_field(
    window: &PreparedWindow,
    canonical: CanonicalField,
) -> Result<InterpolatedField, AirMassDerivationError> {
    interpolate_optional_field_with_source_projection(window, canonical, Ok)?.ok_or(
        AirMassDerivationError::MissingField(FieldKey::Canonical(canonical)),
    )
}

fn interpolate_specific_humidity(
    window: &PreparedWindow,
) -> Result<InterpolatedField, AirMassDerivationError> {
    interpolate_optional_field_with_source_projection(
        window,
        CanonicalField::SpecificHumidity,
        project_source_specific_humidity,
    )?
    .ok_or({
        AirMassDerivationError::MissingField(FieldKey::Canonical(CanonicalField::SpecificHumidity))
    })
}

fn project_source_specific_humidity(value: f64) -> Result<f64, AirMassDerivationError> {
    project_specific_humidity_nonnegative(value)
        .map_err(|_| AirMassDerivationError::InvalidSpecificHumidity)
}

fn interpolate_optional_field(
    window: &PreparedWindow,
    canonical: CanonicalField,
) -> Result<Option<InterpolatedField>, AirMassDerivationError> {
    interpolate_optional_field_with_source_projection(window, canonical, Ok)
}

fn interpolate_optional_field_with_source_projection(
    window: &PreparedWindow,
    canonical: CanonicalField,
    project_source: impl Fn(f64) -> Result<f64, AirMassDerivationError>,
) -> Result<Option<InterpolatedField>, AirMassDerivationError> {
    let key = FieldKey::Canonical(canonical);
    let before = window.frames.before.fields().get(&key);
    let after = window.frames.after.fields().get(&key);
    let (Some(before), Some(after)) = (before, after) else {
        return if before.is_none() && after.is_none() {
            Ok(None)
        } else {
            Err(AirMassDerivationError::IncompatibleFrames)
        };
    };
    if before.layout() != after.layout()
        || before.values().len() != after.values().len()
        || before.validity().len() != after.validity().len()
    {
        return Err(AirMassDerivationError::IncompatibleFrames);
    }
    let mut values = Vec::with_capacity(before.values().len());
    let mut valid = Vec::with_capacity(before.values().len());
    for index in 0..before.values().len() {
        let before_value = project_source(before.values()[index])?;
        let after_value = project_source(after.values()[index])?;
        let value = if window.is_exact_frame() {
            before_value
        } else {
            window
                .after_weight
                .mul_add(after_value, window.before_weight * before_value)
        };
        if !value.is_finite() {
            return Err(AirMassDerivationError::NumericalFailure);
        }
        values.push(value);
        valid.push(
            before.validity().get(index).unwrap_or(false)
                && after.validity().get(index).unwrap_or(false),
        );
    }
    Ok(Some(InterpolatedField {
        values,
        valid,
        layout: before.layout(),
    }))
}

fn interpolate_geometric_terrain_height(
    window: &PreparedWindow,
) -> Result<InterpolatedField, AirMassDerivationError> {
    if let Some(terrain) =
        interpolate_optional_field(window, CanonicalField::GeometricTerrainHeight)?
    {
        return Ok(terrain);
    }
    let mut geopotential = interpolate_required_field(window, CanonicalField::SurfaceGeopotential)?;
    for (index, value) in geopotential.values.iter_mut().enumerate() {
        if geopotential.valid[index] {
            *value = geopotential_to_geometric_height_m(*value)
                .map_err(|_| AirMassDerivationError::InvalidTerrain)?;
        }
    }
    Ok(geopotential)
}

fn interpolate_pressure_geometric_height(
    window: &PreparedWindow,
) -> Result<InterpolatedField, AirMassDerivationError> {
    if let Some(height) = interpolate_optional_field(window, CanonicalField::GeometricHeight)? {
        return Ok(height);
    }
    let mut geopotential_height =
        interpolate_required_field(window, CanonicalField::GeopotentialHeight)?;
    for (index, value) in geopotential_height.values.iter_mut().enumerate() {
        if geopotential_height.valid[index] {
            let geopotential = M3_CONSTANTS.standard_gravity_m_s2 * *value;
            *value = geopotential_to_geometric_height_m(geopotential)
                .map_err(|_| AirMassDerivationError::InvalidHeightColumn)?;
        }
    }
    Ok(geopotential_height)
}

#[derive(Clone, Copy, Debug)]
enum PhysicalField {
    Humidity,
    Temperature,
    Wind,
}

fn validate_field_value(
    values: &[f64],
    index: usize,
    field: PhysicalField,
) -> Result<(), AirMassDerivationError> {
    let value = *values
        .get(index)
        .ok_or(AirMassDerivationError::IndexOverflow)?;
    let valid = match field {
        PhysicalField::Humidity => value.is_finite() && (0.0..1.0).contains(&value),
        PhysicalField::Temperature => value.is_finite() && value > 0.0,
        PhysicalField::Wind => value.is_finite(),
    };
    if valid {
        Ok(())
    } else {
        Err(match field {
            PhysicalField::Humidity => AirMassDerivationError::InvalidSpecificHumidity,
            PhysicalField::Temperature => AirMassDerivationError::InvalidTemperature,
            PhysicalField::Wind => AirMassDerivationError::InvalidWind,
        })
    }
}

fn native_full_levels(
    topology: &VerticalTopology,
    level_count: usize,
) -> Result<Vec<u16>, AirMassDerivationError> {
    match topology {
        VerticalTopology::PressureLevels(levels) => {
            if levels.pressure_pa.len() != level_count || level_count > usize::from(u16::MAX) {
                return Err(AirMassDerivationError::InvalidVerticalTopology);
            }
            (0..level_count)
                .map(|index| {
                    u16::try_from(index + 1)
                        .map_err(|_| AirMassDerivationError::InvalidVerticalTopology)
                })
                .collect()
        }
        VerticalTopology::HybridPressure(topology) => {
            if topology.active_full_levels.len() != level_count {
                return Err(AirMassDerivationError::InvalidVerticalTopology);
            }
            Ok(topology.active_full_levels.to_vec())
        }
    }
}

fn pressure_interfaces_by_point(
    topology: &VerticalTopology,
    surface_pressure_pa: &[f64],
    nx: usize,
    ny: usize,
) -> Result<Vec<Vec<f64>>, AirMassDerivationError> {
    let horizontal = nx
        .checked_mul(ny)
        .ok_or(AirMassDerivationError::IndexOverflow)?;
    if surface_pressure_pa.len() != horizontal {
        return Err(AirMassDerivationError::InvalidLayout);
    }
    match topology {
        VerticalTopology::PressureLevels(levels) => {
            let interfaces = pressure_log_midpoint_interfaces_pa(&levels.pressure_pa)?;
            Ok((0..horizontal).map(|_| interfaces.clone()).collect())
        }
        VerticalTopology::HybridPressure(topology) => {
            let first = usize::from(
                *topology
                    .active_full_levels
                    .first()
                    .ok_or(AirMassDerivationError::InvalidVerticalTopology)?,
            );
            let last = usize::from(
                *topology
                    .active_full_levels
                    .last()
                    .ok_or(AirMassDerivationError::InvalidVerticalTopology)?,
            );
            let complete_levels = topology.coefficients.a_half_pa.len().saturating_sub(1);
            if first == 0
                || last != complete_levels
                || topology.active_full_levels.len() + 1
                    != last.saturating_sub(first).saturating_add(2)
            {
                return Err(AirMassDerivationError::InvalidVerticalTopology);
            }
            surface_pressure_pa
                .iter()
                .map(|surface| {
                    let complete = hybrid_pressure_column(
                        &topology.coefficients.a_half_pa,
                        &topology.coefficients.b_half,
                        *surface,
                    )
                    .map_err(|_| AirMassDerivationError::InvalidVerticalTopology)?;
                    let interfaces = complete
                        .interface_pa
                        .get(first.saturating_sub(1)..=last)
                        .map(<[f64]>::to_vec)
                        .ok_or(AirMassDerivationError::InvalidVerticalTopology)?;
                    if interfaces.len() == topology.active_full_levels.len() + 1 {
                        Ok(interfaces)
                    } else {
                        Err(AirMassDerivationError::InvalidVerticalTopology)
                    }
                })
                .collect()
        }
    }
}

fn pressure_log_midpoint_interfaces_pa(
    centres: &[f64],
) -> Result<Vec<f64>, AirMassDerivationError> {
    if centres.len() < 2
        || centres
            .iter()
            .any(|value| !value.is_finite() || *value <= 0.0)
        || centres.windows(2).any(|pair| pair[1] <= pair[0])
    {
        return Err(AirMassDerivationError::InvalidVerticalTopology);
    }
    let mut interfaces = Vec::with_capacity(centres.len() + 1);
    interfaces.push(centres[0] / (centres[1] / centres[0]).sqrt());
    interfaces.extend(centres.windows(2).map(|pair| (pair[0] * pair[1]).sqrt()));
    let last = centres.len() - 1;
    interfaces.push(centres[last] * (centres[last] / centres[last - 1]).sqrt());
    if interfaces
        .iter()
        .any(|value| !value.is_finite() || *value <= 0.0)
        || interfaces.windows(2).any(|pair| pair[1] <= pair[0])
    {
        return Err(AirMassDerivationError::InvalidVerticalTopology);
    }
    Ok(interfaces)
}

fn validate_height_column(
    heights: &[f64],
    terrain_height_asl_m: f64,
) -> Result<(), AirMassDerivationError> {
    // Pressure-level products legitimately retain monotonically descending
    // full levels below local terrain. The per-layer pressure test below is
    // what clips those nominal cells at surface pressure; requiring the final
    // source level itself to remain above terrain would reject mountain cells.
    if heights.len() < 2
        || !terrain_height_asl_m.is_finite()
        || heights.iter().any(|height| !height.is_finite())
        || heights.windows(2).any(|pair| pair[1] >= pair[0])
    {
        return Err(AirMassDerivationError::InvalidHeightColumn);
    }
    Ok(())
}

fn height_interfaces(
    centres: &[f64],
    terrain_height_asl_m: f64,
) -> Result<Vec<f64>, AirMassDerivationError> {
    validate_height_column(centres, terrain_height_asl_m)?;
    let mut interfaces = Vec::with_capacity(centres.len() + 1);
    // The query engine never extrapolates above its highest valid full level.
    // Keep the pressure-interface mass, but anchor its geometric support at the
    // physical/data model top so seeded particles and boundary faces remain
    // inside the formally queryable vertical domain.
    interfaces.push(centres[0]);
    interfaces.extend(centres.windows(2).map(|pair| 0.5 * (pair[0] + pair[1])));
    interfaces.push(terrain_height_asl_m);
    // Pressure products may retain a monotonically descending tail below
    // terrain. Its internal interfaces are intentionally ignored by the
    // surface-pressure clip; only an actually included layer must prove a
    // positive terrain-relative thickness in the caller above.
    if interfaces.iter().any(|height| !height.is_finite()) {
        return Err(AirMassDerivationError::InvalidHeightColumn);
    }
    Ok(interfaces)
}

fn transport_floor_pressure(
    surface_pressure_pa: f64,
    lowest_full_level_pressure_pa: f64,
    height_fraction: f64,
) -> Result<f64, AirMassDerivationError> {
    if !surface_pressure_pa.is_finite()
        || surface_pressure_pa <= 0.0
        || !lowest_full_level_pressure_pa.is_finite()
        || lowest_full_level_pressure_pa <= 0.0
        || lowest_full_level_pressure_pa > surface_pressure_pa
        || !height_fraction.is_finite()
        || !(0.0..=1.0).contains(&height_fraction)
    {
        return Err(AirMassDerivationError::InvalidPhysicalState);
    }

    // Log-pressure interpolation written as a pressure ratio preserves the
    // exact degenerate anchor `p_level == p_surface`. The previous
    // `exp(lerp(ln p))` round-trip could exceed the surface by one ulp and
    // reject an otherwise physical CFSR column.
    let pressure = surface_pressure_pa
        * (lowest_full_level_pressure_pa / surface_pressure_pa).powf(height_fraction);
    let scale = surface_pressure_pa.max(lowest_full_level_pressure_pa);
    let roundoff = 8.0 * f64::EPSILON * scale;
    if !pressure.is_finite()
        || pressure < lowest_full_level_pressure_pa - roundoff
        || pressure > surface_pressure_pa + roundoff
    {
        return Err(AirMassDerivationError::NumericalFailure);
    }
    Ok(pressure.clamp(lowest_full_level_pressure_pa, surface_pressure_pa))
}

#[derive(Clone, Debug)]
struct SafeCore {
    geometry: DomainGeometry,
    x_indices: Vec<usize>,
    y_indices: Vec<usize>,
    x_first: usize,
    x_last: usize,
    y_first: usize,
    y_last: usize,
}

impl SafeCore {
    fn new(geometry: &DomainGeometry) -> Result<Self, AirMassDerivationError> {
        let halo = geometry.halo_cells;
        let x_first = if geometry.periodic_longitude { 0 } else { halo };
        let x_last = if geometry.periodic_longitude {
            geometry.nx - 1
        } else {
            geometry
                .nx
                .checked_sub(halo + 1)
                .ok_or(AirMassDerivationError::InvalidGeometry)?
        };
        let y_first = halo;
        let y_last = geometry
            .ny
            .checked_sub(halo + 1)
            .ok_or(AirMassDerivationError::InvalidGeometry)?;
        if x_last <= x_first || y_last <= y_first {
            return Err(AirMassDerivationError::InvalidGeometry);
        }
        Ok(Self {
            geometry: geometry.clone(),
            x_indices: (x_first..=x_last).collect(),
            y_indices: (y_first..=y_last).collect(),
            x_first,
            x_last,
            y_first,
            y_last,
        })
    }

    fn x_coordinate(&self, x: usize) -> f64 {
        self.geometry.longitude_origin_degrees + self.geometry.longitude_spacing_degrees * x as f64
    }

    fn y_coordinate(&self, y: usize) -> f64 {
        self.geometry.latitude_origin_degrees + self.geometry.latitude_spacing_degrees * y as f64
    }

    fn x_lower(&self, x: usize) -> Result<f64, AirMassDerivationError> {
        if self.geometry.periodic_longitude {
            Ok(self.x_coordinate(x) - 0.5 * self.geometry.longitude_spacing_degrees)
        } else if x == self.x_first {
            Ok(self.x_coordinate(x))
        } else {
            Ok(0.5 * (self.x_coordinate(x - 1) + self.x_coordinate(x)))
        }
    }

    fn x_upper(&self, x: usize) -> Result<f64, AirMassDerivationError> {
        if self.geometry.periodic_longitude {
            Ok(self.x_coordinate(x) + 0.5 * self.geometry.longitude_spacing_degrees)
        } else if x == self.x_last {
            Ok(self.x_coordinate(x))
        } else {
            Ok(0.5 * (self.x_coordinate(x) + self.x_coordinate(x + 1)))
        }
    }

    fn y_bounds(&self, y: usize) -> Result<(f64, f64), AirMassDerivationError> {
        let lower_index_edge = if y == self.y_first {
            self.y_coordinate(y)
        } else {
            0.5 * (self.y_coordinate(y - 1) + self.y_coordinate(y))
        };
        let upper_index_edge = if y == self.y_last {
            self.y_coordinate(y)
        } else {
            0.5 * (self.y_coordinate(y) + self.y_coordinate(y + 1))
        };
        let south = lower_index_edge.min(upper_index_edge);
        let north = lower_index_edge.max(upper_index_edge);
        if north <= south {
            return Err(AirMassDerivationError::InvalidGeometry);
        }
        Ok((south, north))
    }
}

fn derive_boundary_faces(
    grid: &DomainGeometry,
    safe: &SafeCore,
    columns: &[AirMassColumn],
) -> Result<Vec<BoundaryFaceLayer>, AirMassDerivationError> {
    let mut faces = Vec::new();
    for side in [
        BoundarySide::West,
        BoundarySide::East,
        BoundarySide::South,
        BoundarySide::North,
    ] {
        let selected = columns.iter().filter(|column| match side {
            BoundarySide::West => column.x == safe.x_first,
            BoundarySide::East => column.x == safe.x_last,
            BoundarySide::South => column.y == physical_south_y(safe),
            BoundarySide::North => column.y == physical_north_y(safe),
        });
        for (segment_index, column) in selected.enumerate() {
            let horizontal_edge_length_m = match side {
                BoundarySide::West | BoundarySide::East => {
                    M3_CONSTANTS.earth_radius_m
                        * (column.north_degrees - column.south_degrees).to_radians()
                }
                BoundarySide::South | BoundarySide::North => {
                    let latitude = match side {
                        BoundarySide::South => column.south_degrees,
                        BoundarySide::North => column.north_degrees,
                        BoundarySide::West | BoundarySide::East => unreachable!(),
                    };
                    M3_CONSTANTS.earth_radius_m
                        * latitude.to_radians().cos()
                        * (column.east_degrees - column.west_degrees).to_radians()
                }
            };
            if !horizontal_edge_length_m.is_finite() || horizontal_edge_length_m <= 0.0 {
                return Err(AirMassDerivationError::InvalidGeometry);
            }
            for layer in &column.layers {
                let forward_inward_normal_wind_m_s = match side {
                    BoundarySide::West => layer.eastward_wind_m_s,
                    BoundarySide::East => -layer.eastward_wind_m_s,
                    BoundarySide::South => layer.northward_wind_m_s,
                    BoundarySide::North => -layer.northward_wind_m_s,
                };
                let geometric_face_area_m2 =
                    horizontal_edge_length_m * layer.geometric_thickness_m();
                if !geometric_face_area_m2.is_finite() || geometric_face_area_m2 <= 0.0 {
                    return Err(AirMassDerivationError::NumericalFailure);
                }
                let face_id = pack_boundary_face_id(side, segment_index, layer.level_index)?;
                let (
                    longitude_degrees,
                    latitude_degrees,
                    tangential_lower_degrees,
                    tangential_upper_degrees,
                ) = match side {
                    BoundarySide::West => (
                        safe.x_coordinate(safe.x_first),
                        column.latitude_degrees,
                        column.south_degrees,
                        column.north_degrees,
                    ),
                    BoundarySide::East => (
                        safe.x_coordinate(safe.x_last),
                        column.latitude_degrees,
                        column.south_degrees,
                        column.north_degrees,
                    ),
                    BoundarySide::South => (
                        column.longitude_degrees,
                        column.south_degrees,
                        column.west_degrees,
                        column.east_degrees,
                    ),
                    BoundarySide::North => (
                        column.longitude_degrees,
                        column.north_degrees,
                        column.west_degrees,
                        column.east_degrees,
                    ),
                };
                faces.push(BoundaryFaceLayer {
                    face_id,
                    side,
                    segment_index,
                    level_index: layer.level_index,
                    native_full_level: layer.native_full_level,
                    longitude_degrees,
                    latitude_degrees,
                    tangential_lower_degrees,
                    tangential_upper_degrees,
                    upper_pressure_pa: layer.upper_pressure_pa,
                    lower_pressure_pa: layer.lower_pressure_pa,
                    upper_height_asl_m: layer.upper_height_asl_m,
                    lower_height_asl_m: layer.lower_height_asl_m,
                    potential_vorticity_pvu: layer.potential_vorticity_pvu,
                    air_temperature_k: layer.air_temperature_k,
                    specific_humidity: layer.specific_humidity,
                    forward_inward_normal_wind_m_s,
                    horizontal_edge_length_m,
                    geometric_face_area_m2,
                });
            }
        }
    }
    let mut ids = faces.iter().map(|face| face.face_id).collect::<Vec<_>>();
    ids.sort_unstable();
    if ids.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(AirMassDerivationError::IndexOverflow);
    }
    let _ = grid;
    Ok(faces)
}

fn physical_south_y(safe: &SafeCore) -> usize {
    if safe.geometry.latitude_spacing_degrees > 0.0 {
        safe.y_first
    } else {
        safe.y_last
    }
}

fn physical_north_y(safe: &SafeCore) -> usize {
    if safe.geometry.latitude_spacing_degrees > 0.0 {
        safe.y_last
    } else {
        safe.y_first
    }
}

fn pack_boundary_face_id(
    side: BoundarySide,
    segment_index: usize,
    level_index: usize,
) -> Result<u64, AirMassDerivationError> {
    const SEGMENT_BITS: u32 = 31;
    const LEVEL_BITS: u32 = 30;
    let segment =
        u64::try_from(segment_index).map_err(|_| AirMassDerivationError::IndexOverflow)?;
    let level = u64::try_from(level_index).map_err(|_| AirMassDerivationError::IndexOverflow)?;
    if segment >= (1_u64 << SEGMENT_BITS) || level >= (1_u64 << LEVEL_BITS) {
        return Err(AirMassDerivationError::IndexOverflow);
    }
    let packed =
        (side.packed_tag() << (SEGMENT_BITS + LEVEL_BITS)) | (segment << LEVEL_BITS) | level;
    if packed > i64::MAX as u64 {
        return Err(AirMassDerivationError::IndexOverflow);
    }
    Ok(packed)
}

fn spherical_cell_area_m2(
    west_degrees: f64,
    east_degrees: f64,
    south_degrees: f64,
    north_degrees: f64,
) -> Result<f64, AirMassDerivationError> {
    let span = east_degrees - west_degrees;
    if ![west_degrees, east_degrees, south_degrees, north_degrees]
        .into_iter()
        .all(f64::is_finite)
        || !(0.0..=360.0).contains(&span)
        || span == 0.0
        || !(-90.0..=90.0).contains(&south_degrees)
        || !(-90.0..=90.0).contains(&north_degrees)
        || north_degrees <= south_degrees
    {
        return Err(AirMassDerivationError::InvalidGeometry);
    }
    let area = M3_CONSTANTS.earth_radius_m.powi(2)
        * span.to_radians()
        * (north_degrees.to_radians().sin() - south_degrees.to_radians().sin());
    if area.is_finite() && area > 0.0 {
        Ok(area)
    } else {
        Err(AirMassDerivationError::InvalidGeometry)
    }
}

fn dry_air_density_kg_m3(
    pressure_pa: f64,
    temperature_k: f64,
    specific_humidity: f64,
) -> Result<f64, AirMassDerivationError> {
    if !pressure_pa.is_finite()
        || pressure_pa <= 0.0
        || !temperature_k.is_finite()
        || temperature_k <= 0.0
        || !specific_humidity.is_finite()
        || !(0.0..1.0).contains(&specific_humidity)
    {
        return Err(AirMassDerivationError::InvalidPhysicalState);
    }
    let gas_constant = (1.0 - specific_humidity) * M3_CONSTANTS.dry_air_gas_constant_j_kg_k
        + specific_humidity * M3_CONSTANTS.water_vapour_gas_constant_j_kg_k;
    let density = (1.0 - specific_humidity) * pressure_pa / (gas_constant * temperature_k);
    if density.is_finite() && density >= 0.0 {
        Ok(density)
    } else {
        Err(AirMassDerivationError::NumericalFailure)
    }
}

fn neumaier_sum<I>(values: I) -> Result<f64, AirMassDerivationError>
where
    I: IntoIterator<Item = f64>,
{
    let mut sum = 0.0_f64;
    let mut correction = 0.0_f64;
    for value in values {
        if !value.is_finite() {
            return Err(AirMassDerivationError::NumericalFailure);
        }
        let next = sum + value;
        if sum.abs() >= value.abs() {
            correction += (sum - next) + value;
        } else {
            correction += (value - next) + sum;
        }
        sum = next;
    }
    let result = sum + correction;
    if result.is_finite() {
        Ok(result)
    } else {
        Err(AirMassDerivationError::NumericalFailure)
    }
}

fn horizontal_index(x: usize, y: usize, nx: usize) -> Result<usize, AirMassDerivationError> {
    y.checked_mul(nx)
        .and_then(|row| row.checked_add(x))
        .ok_or(AirMassDerivationError::IndexOverflow)
}

fn full_index(
    level: usize,
    horizontal: usize,
    nx: usize,
    ny: usize,
) -> Result<usize, AirMassDerivationError> {
    let horizontal_count = nx
        .checked_mul(ny)
        .ok_or(AirMassDerivationError::IndexOverflow)?;
    level
        .checked_mul(horizontal_count)
        .and_then(|base| base.checked_add(horizontal))
        .ok_or(AirMassDerivationError::IndexOverflow)
}

fn field_column(
    values: &[f64],
    horizontal: usize,
    levels: usize,
    nx: usize,
    ny: usize,
) -> Result<Vec<f64>, AirMassDerivationError> {
    (0..levels)
        .map(|level| {
            let index = full_index(level, horizontal, nx, ny)?;
            values
                .get(index)
                .copied()
                .ok_or(AirMassDerivationError::IndexOverflow)
        })
        .collect()
}

/// Dry-air finite-volume derivation failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AirMassDerivationError {
    /// Required canonical source field is absent.
    MissingField(FieldKey),
    /// Bracketing frames disagree structurally.
    IncompatibleFrames,
    /// A source field has the wrong canonical array layout.
    InvalidLayout,
    /// A required source field marks at least one value invalid.
    InvalidFieldMask,
    /// Surface pressure is missing, non-finite, or non-positive.
    InvalidSurfacePressure,
    /// Terrain is missing or non-finite.
    InvalidTerrain,
    /// Specific humidity is outside `[0, 1)`.
    InvalidSpecificHumidity,
    /// Air temperature is non-finite or non-positive.
    InvalidTemperature,
    /// A wind component is non-finite.
    InvalidWind,
    /// Pressure or hybrid topology violates the top-to-bottom contract.
    InvalidVerticalTopology,
    /// Full/interface height is non-finite or non-monotonic.
    InvalidHeightColumn,
    /// No native layer remains above the local surface.
    NoAboveGroundLayers,
    /// Safe-core or spherical control-volume geometry is invalid.
    InvalidGeometry,
    /// Packed or flat integer identity overflowed.
    IndexOverflow,
    /// A physical scalar used by density or flux is invalid.
    InvalidPhysicalState,
    /// Finite arithmetic failed to produce a physical result.
    NumericalFailure,
}

impl AirMassDerivationError {
    /// Returns the stable machine-readable error code.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::MissingField(_) => "domain_fill.missing_field",
            Self::IncompatibleFrames => "domain_fill.incompatible_frames",
            Self::InvalidLayout => "domain_fill.invalid_layout",
            Self::InvalidFieldMask => "domain_fill.invalid_field_mask",
            Self::InvalidSurfacePressure => "domain_fill.invalid_surface_pressure",
            Self::InvalidTerrain => "domain_fill.invalid_terrain",
            Self::InvalidSpecificHumidity => "domain_fill.invalid_specific_humidity",
            Self::InvalidTemperature => "domain_fill.invalid_temperature",
            Self::InvalidWind => "domain_fill.invalid_wind",
            Self::InvalidVerticalTopology => "domain_fill.invalid_vertical_topology",
            Self::InvalidHeightColumn => "domain_fill.invalid_height_column",
            Self::NoAboveGroundLayers => "domain_fill.no_above_ground_layers",
            Self::InvalidGeometry => "domain_fill.invalid_geometry",
            Self::IndexOverflow => "domain_fill.index_overflow",
            Self::InvalidPhysicalState => "domain_fill.invalid_physical_state",
            Self::NumericalFailure => "domain_fill.numerical_failure",
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use std::sync::Arc;

    use super::*;
    use crate::field::FieldQuality;
    use crate::frame::{FrameMetadata, PreparedWindow, RawField, RawFieldStore, RawMetFrame};
    use crate::io::inventory::LogicalFrameId;
    use crate::profile::graph::GraphUnit;
    use crate::provenance::{ProvenanceId, ProvenanceRecord, ProvenanceTable};
    use crate::vertical::{HybridCoefficients, HybridPressureTopology, PressureLevels};
    use trajecta_case::model::time::Timestamp;

    fn timestamp(seconds: i64) -> Timestamp {
        Timestamp::new(seconds, 0).unwrap()
    }

    fn field(
        values: Vec<f64>,
        valid: Vec<bool>,
        layout: ArrayLayout,
        time: Timestamp,
        unit: GraphUnit,
        provenance: ProvenanceId,
    ) -> RawField {
        RawField::new(
            Arc::from(values),
            Arc::from(valid),
            unit,
            layout,
            crate::frame::TemporalSupport::Instantaneous { valid_time: time },
            FieldQuality::Source,
            provenance,
        )
        .unwrap()
    }

    fn frame(
        time: Timestamp,
        vertical: VerticalTopology,
        surface_pressure: f64,
        q: &[f64],
        eastward: f64,
    ) -> Arc<RawMetFrame> {
        frame_with_terrain(time, vertical, surface_pressure, q, eastward, 0.0)
    }

    fn frame_with_terrain(
        time: Timestamp,
        vertical: VerticalTopology,
        surface_pressure: f64,
        q: &[f64],
        eastward: f64,
        terrain_height_asl_m: f64,
    ) -> Arc<RawMetFrame> {
        let domain = DomainId("finite".into());
        let grid = DomainGeometry {
            domain: domain.clone(),
            longitude_origin_degrees: 0.0,
            latitude_origin_degrees: 0.0,
            longitude_spacing_degrees: 1.0,
            latitude_spacing_degrees: 1.0,
            nx: 2,
            ny: 2,
            periodic_longitude: false,
            halo_cells: 0,
        };
        let levels = q.len();
        let horizontal = 4;
        let full = ArrayLayout::Full3D {
            levels,
            ny: 2,
            nx: 2,
        };
        let horizontal_layout = ArrayLayout::Horizontal2D { ny: 2, nx: 2 };
        let id = LogicalFrameId {
            domain: domain.clone(),
            valid_time: time,
            profile_sha256: "11".repeat(32),
            content_sha256: format!("frame-{time:?}"),
        };
        let mut table = ProvenanceTable::new();
        let mut fields = RawFieldStore::new();
        let definitions = [
            (
                CanonicalField::SurfacePressure,
                vec![surface_pressure; horizontal],
                horizontal_layout,
                GraphUnit::parse("Pa").unwrap(),
            ),
            (
                CanonicalField::GeometricTerrainHeight,
                vec![terrain_height_asl_m; horizontal],
                horizontal_layout,
                GraphUnit::parse("m").unwrap(),
            ),
            (
                CanonicalField::SurfaceGeopotential,
                vec![0.0; horizontal],
                horizontal_layout,
                GraphUnit::parse("m2 s-2").unwrap(),
            ),
            (
                CanonicalField::AerodynamicRoughnessLength,
                vec![0.1; horizontal],
                horizontal_layout,
                GraphUnit::parse("m").unwrap(),
            ),
            (
                CanonicalField::GeometricHeight,
                (0..levels)
                    .flat_map(|level| vec![3_000.0 - level as f64 * 1_000.0; horizontal])
                    .collect(),
                full,
                GraphUnit::parse("m").unwrap(),
            ),
            (
                CanonicalField::SpecificHumidity,
                q.iter()
                    .flat_map(|value| vec![*value; horizontal])
                    .collect(),
                full,
                GraphUnit::parse("1").unwrap(),
            ),
            (
                CanonicalField::AirTemperature,
                vec![280.0; levels * horizontal],
                full,
                GraphUnit::parse("K").unwrap(),
            ),
            (
                CanonicalField::EastwardWind,
                vec![eastward; levels * horizontal],
                full,
                GraphUnit::parse("m/s").unwrap(),
            ),
            (
                CanonicalField::NorthwardWind,
                vec![0.0; levels * horizontal],
                full,
                GraphUnit::parse("m/s").unwrap(),
            ),
        ];
        for (canonical, values, layout, unit) in definitions {
            let key = FieldKey::Canonical(canonical);
            let provenance = table
                .intern(ProvenanceRecord {
                    field: key.clone(),
                    quality: FieldQuality::Source,
                    sources: vec![id.content_sha256.clone()],
                    transforms: Vec::new(),
                    fallback_reason: None,
                    profile_sha256: id.profile_sha256.clone(),
                })
                .unwrap();
            let len = values.len();
            fields
                .insert(
                    key,
                    field(values, vec![true; len], layout, time, unit, provenance),
                )
                .unwrap();
        }
        Arc::new(
            RawMetFrame::publish(
                FrameMetadata {
                    id,
                    domain,
                    valid_time: time,
                    grid,
                    vertical,
                },
                fields,
                Arc::new(table),
            )
            .unwrap(),
        )
    }

    fn exact_window(frame: Arc<RawMetFrame>) -> PreparedWindow {
        PreparedWindow::at_frame(None, frame, None, 1_000_000).unwrap()
    }

    #[test]
    fn transport_floor_pressure_preserves_equal_cfsr_surface_anchor() {
        let pressure =
            transport_floor_pressure(100_000.0, 100_000.0, 0.5 / 6.414_508_582_405_448).unwrap();
        assert_eq!(pressure, 100_000.0);
        assert!(pressure > 98_742.088_290_657_5);
    }

    #[test]
    fn pressure_snapshot_clips_underground_and_sums_safe_core() {
        let frame = frame(
            timestamp(0),
            VerticalTopology::PressureLevels(
                PressureLevels::new(Arc::from([100.0, 400.0, 1_600.0])).unwrap(),
            ),
            1_000.0,
            &[0.0, 0.0, 0.0],
            2.0,
        );
        let snapshot = AirMassDeriver.derive_window(&exact_window(frame)).unwrap();
        assert_eq!(snapshot.columns.len(), 4);
        assert_eq!(snapshot.columns[0].layers.len(), 2);
        assert_eq!(
            snapshot.columns[0].layers[1].lower_pressure_pa,
            snapshot.columns[0].transport_floor_pressure_pa
        );
        assert_eq!(
            snapshot.columns[0].layers[1].lower_height_asl_m,
            snapshot.columns[0].transport_floor_height_asl_m
        );
        let area = M3_CONSTANTS.earth_radius_m.powi(2)
            * (1.0_f64).to_radians()
            * ((1.0_f64).to_radians().sin() - 0.0_f64.sin());
        let expected = area * (snapshot.columns[0].transport_floor_pressure_pa - 50.0)
            / M3_CONSTANTS.standard_gravity_m_s2;
        assert!((snapshot.total_dry_air_mass_kg - expected).abs() <= expected * 2.0e-15);
        assert_eq!(snapshot.boundary_faces.len(), 4 * 2 * 2);
    }

    #[test]
    fn pressure_snapshot_uses_last_jointly_above_ground_full_level_as_bottom_layer() {
        let frame = frame_with_terrain(
            timestamp(0),
            VerticalTopology::PressureLevels(
                PressureLevels::new(Arc::from([100.0, 400.0, 1_600.0])).unwrap(),
            ),
            2_000.0,
            &[0.0, 0.0, 0.0],
            0.0,
            1_800.0,
        );
        let snapshot = AirMassDeriver.derive_window(&exact_window(frame)).unwrap();
        for column in &snapshot.columns {
            assert_eq!(column.layers.len(), 2);
            let bottom = column.layers.last().unwrap();
            assert_eq!(bottom.level_index, 1);
            assert_eq!(bottom.lower_pressure_pa, column.transport_floor_pressure_pa);
            assert_eq!(
                bottom.lower_height_asl_m,
                column.transport_floor_height_asl_m
            );
            assert!(
                column.layers.iter().all(|layer| {
                    layer.lower_height_asl_m >= column.transport_floor_height_asl_m
                })
            );
        }
    }

    #[test]
    fn hybrid_uses_native_interfaces_and_direction_reverses_inflow() {
        let topology = VerticalTopology::HybridPressure(HybridPressureTopology {
            coefficients: HybridCoefficients {
                a_half_pa: Arc::from([100.0, 100.0, 0.0]),
                b_half: Arc::from([0.0, 0.4, 1.0]),
            },
            active_full_levels: Arc::from([1_u16, 2]),
        });
        let frame = frame(timestamp(0), topology, 1_000.0, &[0.1, 0.2], 5.0);
        let snapshot = AirMassDeriver.derive_window(&exact_window(frame)).unwrap();
        let first = &snapshot.columns[0];
        assert_eq!(first.layers[0].upper_pressure_pa, 100.0);
        assert_eq!(first.layers[0].lower_pressure_pa, 500.0);
        assert_eq!(
            first.layers[1].lower_pressure_pa,
            first.transport_floor_pressure_pa
        );
        let expected_geopotential = hydrostatic_full_level_geopotential(
            &[100.0, 500.0, 1_000.0],
            &[280.0; 2],
            &[0.1, 0.2],
            0.0,
        )
        .unwrap();
        let expected_top =
            geopotential_to_geometric_height_m(expected_geopotential.full_level_m2_s2[0]).unwrap();
        assert!((first.layers[0].upper_height_asl_m - expected_top).abs() < 1.0e-12);
        assert_ne!(first.layers[0].upper_height_asl_m, 3_000.0);
        let west = snapshot
            .boundary_faces
            .iter()
            .find(|face| face.side == BoundarySide::West)
            .unwrap();
        assert!(west.inflow_rate_kg_s(Direction::Forward).unwrap() > 0.0);
        assert_eq!(west.inflow_rate_kg_s(Direction::Backward).unwrap(), 0.0);
        let east = snapshot
            .boundary_faces
            .iter()
            .find(|face| face.side == BoundarySide::East)
            .unwrap();
        assert_eq!(east.inflow_rate_kg_s(Direction::Forward).unwrap(), 0.0);
        assert!(east.inflow_rate_kg_s(Direction::Backward).unwrap() > 0.0);
    }

    #[test]
    fn invalid_humidity_and_missing_mask_are_hard_failures() {
        let bad = frame(
            timestamp(0),
            VerticalTopology::PressureLevels(
                PressureLevels::new(Arc::from([100.0, 400.0])).unwrap(),
            ),
            1_000.0,
            &[0.0, 1.0],
            0.0,
        );
        assert_eq!(
            AirMassDeriver.derive_window(&exact_window(bad)),
            Err(AirMassDerivationError::InvalidSpecificHumidity)
        );
    }

    #[test]
    fn between_frame_snapshot_uses_declared_linear_time_weights() {
        let before = frame(
            timestamp(0),
            VerticalTopology::PressureLevels(
                PressureLevels::new(Arc::from([100.0, 400.0])).unwrap(),
            ),
            800.0,
            &[0.0, 0.0],
            0.0,
        );
        let after = frame(
            timestamp(10),
            before.metadata().vertical.clone(),
            1_000.0,
            &[0.0, 0.0],
            0.0,
        );
        let window = PreparedWindow::between(before, after, timestamp(5), 1_000_000).unwrap();
        let snapshot = AirMassDeriver.derive_window(&window).unwrap();
        assert!(
            snapshot
                .columns
                .iter()
                .all(|column| column.surface_pressure_pa == 900.0)
        );
    }

    #[test]
    fn negative_source_humidity_is_projected_before_time_interpolation() {
        let before = frame(
            timestamp(0),
            VerticalTopology::PressureLevels(
                PressureLevels::new(Arc::from([100.0, 400.0])).unwrap(),
            ),
            800.0,
            &[-0.01, -0.02],
            0.0,
        );
        let after = frame(
            timestamp(10),
            before.metadata().vertical.clone(),
            1_000.0,
            &[0.03, 0.04],
            0.0,
        );
        let window = PreparedWindow::between(before, after, timestamp(5), 1_000_000).unwrap();
        let snapshot = AirMassDeriver.derive_window(&window).unwrap();
        for column in &snapshot.columns {
            assert_eq!(column.layers[0].specific_humidity, 0.015);
            assert_eq!(column.layers[1].specific_humidity, 0.02);
        }
    }
}
