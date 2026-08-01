//! # Contract: horizontal grids and spherical geometry
//!
//! M3 executes on regular latitude-longitude grids. Location and weights
//! handle global periodic longitude, the date line, signed latitude order,
//! exact poles, latitude bounds, and a declared safe halo. Wind interpolation
//! converts local components to Earth-centred tangent vectors before applying
//! scalar weights and projecting into the query-point basis.

use trajecta_case::model::meteorology::DomainId;

/// Integer horizontal grid point.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct GridPoint {
    /// Zero-based x or longitude index.
    pub x: usize,
    /// Zero-based y or latitude index.
    pub y: usize,
}

/// Stable horizontal cell identifier encoded as `y * nx + x`.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct CellId(pub u64);

/// Horizontal interpolation support and weights.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HorizontalWeights {
    /// Storage-order corners: `(y0,x0)`, `(y0,x1)`, `(y1,x0)`, `(y1,x1)`.
    pub points: [GridPoint; 4],
    /// Bilinear weights in the same deterministic order.
    pub weights: [f64; 4],
    /// Structural validity mask replaced by field-level validity at execution.
    pub valid: [bool; 4],
}

/// Horizontal domain geometry and safe interpolation bounds.
#[derive(Clone, Debug, PartialEq)]
pub struct DomainGeometry {
    /// Stable domain identifier.
    pub domain: DomainId,
    /// First stored grid longitude in degrees east.
    pub longitude_origin_degrees: f64,
    /// First stored grid latitude in degrees north.
    pub latitude_origin_degrees: f64,
    /// Positive longitude spacing in degrees.
    pub longitude_spacing_degrees: f64,
    /// Signed latitude spacing in degrees.
    pub latitude_spacing_degrees: f64,
    /// Number of longitude points.
    pub nx: usize,
    /// Number of latitude points.
    pub ny: usize,
    /// Whether longitude is periodic without a duplicated endpoint.
    pub periodic_longitude: bool,
    /// Required safe halo in grid cells at non-periodic boundaries.
    pub halo_cells: usize,
}

impl DomainGeometry {
    /// Validates regular-grid topology and the declared safe interior.
    pub fn validate(&self) -> Result<(), GridError> {
        if self.nx < 2 || self.ny < 2 {
            return Err(GridError::InvalidGeometry(
                "regular grid requires at least two points per axis".into(),
            ));
        }
        if !self.longitude_origin_degrees.is_finite()
            || !self.latitude_origin_degrees.is_finite()
            || !self.longitude_spacing_degrees.is_finite()
            || self.longitude_spacing_degrees <= 0.0
            || !self.latitude_spacing_degrees.is_finite()
            || self.latitude_spacing_degrees == 0.0
        {
            return Err(GridError::InvalidGeometry(
                "grid origins and spacings must be finite with non-zero spacing".into(),
            ));
        }
        let final_latitude = self.latitude_origin_degrees
            + self.latitude_spacing_degrees * (self.ny.saturating_sub(1) as f64);
        if !final_latitude.is_finite()
            || self.latitude_origin_degrees.abs() > 90.0
            || final_latitude.abs() > 90.0
        {
            return Err(GridError::InvalidGeometry(
                "stored latitude range must remain within [-90, 90]".into(),
            ));
        }
        if self.periodic_longitude {
            let circumference = self.longitude_spacing_degrees * self.nx as f64;
            if (circumference - 360.0).abs() > 1.0e-9 * 360.0 {
                return Err(GridError::InvalidGeometry(
                    "periodic longitude must cover 360 degrees without a duplicate endpoint".into(),
                ));
            }
        }
        let required_points = self
            .halo_cells
            .checked_mul(2)
            .and_then(|halo| halo.checked_add(2))
            .ok_or_else(|| GridError::InvalidGeometry("halo size overflow".into()))?;
        if self.ny < required_points || (!self.periodic_longitude && self.nx < required_points) {
            return Err(GridError::InvalidGeometry(
                "grid is too small for its declared interpolation halo".into(),
            ));
        }
        Ok(())
    }
}

/// Format-neutral horizontal grid operations.
pub trait GridBackend: Send + Sync {
    /// Returns immutable domain geometry.
    fn geometry(&self) -> &DomainGeometry;

    /// Locates the containing cell for a geographic point.
    fn locate_cell(
        &self,
        longitude_degrees: f64,
        latitude_degrees: f64,
    ) -> Result<CellId, GridError>;

    /// Computes deterministic interpolation support and weights.
    fn horizontal_weights(
        &self,
        longitude_degrees: f64,
        latitude_degrees: f64,
    ) -> Result<HorizontalWeights, GridError>;
}

/// M3 regular latitude-longitude grid backend.
#[derive(Clone, Debug, PartialEq)]
pub struct RegularLatLonGrid {
    geometry: DomainGeometry,
}

impl RegularLatLonGrid {
    /// Creates a backend from validated geometry.
    pub fn new(geometry: DomainGeometry) -> Result<Self, GridError> {
        geometry.validate()?;
        Ok(Self { geometry })
    }

    /// Locates a point and computes its interpolation support in one pass.
    ///
    /// Small boundary batches need both values for every probe. Returning them
    /// together avoids repeating coordinate normalization and halo checks.
    pub fn locate_cell_and_weights(
        &self,
        longitude_degrees: f64,
        latitude_degrees: f64,
    ) -> Result<(CellId, HorizontalWeights), GridError> {
        let cell = self.locate_fractional(longitude_degrees, latitude_degrees)?;
        let cell_id = encode_cell(cell.x0, cell.y0, self.geometry.nx)?;
        Ok((cell_id, horizontal_weights(cell)))
    }

    fn locate_fractional(
        &self,
        longitude_degrees: f64,
        latitude_degrees: f64,
    ) -> Result<FractionalCell, GridError> {
        if !longitude_degrees.is_finite() || !latitude_degrees.is_finite() {
            return Err(GridError::InvalidCoordinate);
        }
        if !(-90.0..=90.0).contains(&latitude_degrees) {
            return Err(GridError::OutOfDomain);
        }
        if latitude_degrees == 90.0 || latitude_degrees == -90.0 {
            return Err(GridError::PolarSingularity);
        }

        let (x0, x1, fx) = if self.geometry.periodic_longitude {
            let delta =
                (longitude_degrees - self.geometry.longitude_origin_degrees).rem_euclid(360.0);
            let fractional = delta / self.geometry.longitude_spacing_degrees;
            let x0 = (fractional.floor() as usize) % self.geometry.nx;
            let x1 = (x0 + 1) % self.geometry.nx;
            (x0, x1, fractional - fractional.floor())
        } else {
            let fractional = (longitude_degrees - self.geometry.longitude_origin_degrees)
                / self.geometry.longitude_spacing_degrees;
            let (x0, fx) =
                safe_cell_fraction(fractional, self.geometry.nx, self.geometry.halo_cells)?;
            (x0, x0 + 1, fx)
        };

        let latitude_fractional = (latitude_degrees - self.geometry.latitude_origin_degrees)
            / self.geometry.latitude_spacing_degrees;
        let (y0, fy) = safe_cell_fraction(
            latitude_fractional,
            self.geometry.ny,
            self.geometry.halo_cells,
        )?;
        let y1 = y0 + 1;
        Ok(FractionalCell {
            x0,
            x1,
            y0,
            y1,
            fx,
            fy,
        })
    }
}

impl GridBackend for RegularLatLonGrid {
    fn geometry(&self) -> &DomainGeometry {
        &self.geometry
    }

    fn locate_cell(
        &self,
        longitude_degrees: f64,
        latitude_degrees: f64,
    ) -> Result<CellId, GridError> {
        let cell = self.locate_fractional(longitude_degrees, latitude_degrees)?;
        encode_cell(cell.x0, cell.y0, self.geometry.nx)
    }

    fn horizontal_weights(
        &self,
        longitude_degrees: f64,
        latitude_degrees: f64,
    ) -> Result<HorizontalWeights, GridError> {
        let cell = self.locate_fractional(longitude_degrees, latitude_degrees)?;
        Ok(horizontal_weights(cell))
    }
}

#[derive(Clone, Copy, Debug)]
struct FractionalCell {
    x0: usize,
    x1: usize,
    y0: usize,
    y1: usize,
    fx: f64,
    fy: f64,
}

fn horizontal_weights(cell: FractionalCell) -> HorizontalWeights {
    let one_minus_x = 1.0 - cell.fx;
    let one_minus_y = 1.0 - cell.fy;
    HorizontalWeights {
        points: [
            GridPoint {
                x: cell.x0,
                y: cell.y0,
            },
            GridPoint {
                x: cell.x1,
                y: cell.y0,
            },
            GridPoint {
                x: cell.x0,
                y: cell.y1,
            },
            GridPoint {
                x: cell.x1,
                y: cell.y1,
            },
        ],
        weights: [
            one_minus_x * one_minus_y,
            cell.fx * one_minus_y,
            one_minus_x * cell.fy,
            cell.fx * cell.fy,
        ],
        valid: [true; 4],
    }
}

fn safe_cell_fraction(
    fractional: f64,
    point_count: usize,
    halo_cells: usize,
) -> Result<(usize, f64), GridError> {
    if !fractional.is_finite() {
        return Err(GridError::InvalidCoordinate);
    }
    let first = halo_cells as f64;
    let last_index = point_count
        .checked_sub(halo_cells)
        .and_then(|value| value.checked_sub(1))
        .ok_or_else(|| GridError::InvalidGeometry("halo exceeds grid bounds".into()))?;
    if last_index <= halo_cells {
        return Err(GridError::InvalidGeometry(
            "safe interpolation region requires two points".into(),
        ));
    }
    let last = last_index as f64;
    let tolerance = 32.0 * f64::EPSILON * (point_count.saturating_sub(1) as f64).max(1.0);
    if fractional < first - tolerance || fractional > last + tolerance {
        return Err(GridError::OutOfDomain);
    }
    let clamped = fractional.clamp(first, last);
    if clamped == last {
        return Ok((last_index - 1, 1.0));
    }
    let lower = clamped.floor() as usize;
    Ok((lower, clamped - lower as f64))
}

fn encode_cell(x: usize, y: usize, nx: usize) -> Result<CellId, GridError> {
    let flat = y
        .checked_mul(nx)
        .and_then(|row| row.checked_add(x))
        .ok_or(GridError::IndexOverflow)?;
    u64::try_from(flat)
        .map(CellId)
        .map_err(|_| GridError::IndexOverflow)
}

/// Local orthonormal east/north basis embedded in Earth-centred coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SphericalBasis {
    /// East unit vector.
    pub east: [f64; 3],
    /// North unit vector.
    pub north: [f64; 3],
}

impl SphericalBasis {
    /// Constructs the conventional longitude-labelled local tangent basis.
    pub fn at_degrees(longitude_degrees: f64, latitude_degrees: f64) -> Result<Self, GridError> {
        if !longitude_degrees.is_finite()
            || !latitude_degrees.is_finite()
            || !(-90.0..=90.0).contains(&latitude_degrees)
        {
            return Err(GridError::InvalidCoordinate);
        }
        let longitude = longitude_degrees.to_radians();
        let latitude = latitude_degrees.to_radians();
        let (sin_lon, cos_lon) = longitude.sin_cos();
        let (sin_lat, cos_lat) = latitude.sin_cos();
        Ok(Self {
            east: [-sin_lon, cos_lon, 0.0],
            north: [-sin_lat * cos_lon, -sin_lat * sin_lon, cos_lat],
        })
    }

    /// Converts local east/north components into an Earth-centred tangent vector.
    #[must_use]
    pub fn embed(self, eastward: f64, northward: f64) -> [f64; 3] {
        [
            eastward.mul_add(self.east[0], northward * self.north[0]),
            eastward.mul_add(self.east[1], northward * self.north[1]),
            eastward.mul_add(self.east[2], northward * self.north[2]),
        ]
    }

    /// Projects an Earth-centred vector into this local east/north basis.
    #[must_use]
    pub fn project(self, vector: [f64; 3]) -> (f64, f64) {
        (dot(vector, self.east), dot(vector, self.north))
    }
}

/// Interpolates four local wind vectors in Earth-centred space.
pub fn interpolate_spherical_vector(
    source_longitude_degrees: [f64; 4],
    source_latitude_degrees: [f64; 4],
    eastward_m_s: [f64; 4],
    northward_m_s: [f64; 4],
    weights: [f64; 4],
    query_longitude_degrees: f64,
    query_latitude_degrees: f64,
) -> Result<(f64, f64), GridError> {
    if source_longitude_degrees
        .into_iter()
        .chain(source_latitude_degrees)
        .any(|value| !value.is_finite())
    {
        return Err(GridError::InvalidCoordinate);
    }
    if query_latitude_degrees == 90.0 || query_latitude_degrees == -90.0 {
        return Err(GridError::PolarSingularity);
    }
    validate_spherical_vector_values(eastward_m_s, northward_m_s, weights)?;
    let source_bases = [
        SphericalBasis::at_degrees(source_longitude_degrees[0], source_latitude_degrees[0])?,
        SphericalBasis::at_degrees(source_longitude_degrees[1], source_latitude_degrees[1])?,
        SphericalBasis::at_degrees(source_longitude_degrees[2], source_latitude_degrees[2])?,
        SphericalBasis::at_degrees(source_longitude_degrees[3], source_latitude_degrees[3])?,
    ];
    let query_basis = SphericalBasis::at_degrees(query_longitude_degrees, query_latitude_degrees)?;
    let result = interpolate_spherical_vector_in_bases(
        &source_bases,
        query_basis,
        eastward_m_s,
        northward_m_s,
        weights,
    );
    if !result.0.is_finite() || !result.1.is_finite() {
        return Err(GridError::NumericalFailure);
    }
    Ok(result)
}

pub(crate) fn spherical_vector_query_basis(
    longitude_degrees: f64,
    latitude_degrees: f64,
) -> Result<SphericalBasis, GridError> {
    if latitude_degrees == 90.0 || latitude_degrees == -90.0 {
        return Err(GridError::PolarSingularity);
    }
    SphericalBasis::at_degrees(longitude_degrees, latitude_degrees)
}

pub(crate) fn interpolate_spherical_vector_with_bases(
    source_bases: &[SphericalBasis; 4],
    query_basis: SphericalBasis,
    eastward_m_s: [f64; 4],
    northward_m_s: [f64; 4],
    weights: [f64; 4],
) -> Result<(f64, f64), GridError> {
    validate_spherical_vector_values(eastward_m_s, northward_m_s, weights)?;
    let result = interpolate_spherical_vector_in_bases(
        source_bases,
        query_basis,
        eastward_m_s,
        northward_m_s,
        weights,
    );
    if !result.0.is_finite() || !result.1.is_finite() {
        return Err(GridError::NumericalFailure);
    }
    Ok(result)
}

fn validate_spherical_vector_values(
    eastward_m_s: [f64; 4],
    northward_m_s: [f64; 4],
    weights: [f64; 4],
) -> Result<(), GridError> {
    if eastward_m_s
        .into_iter()
        .chain(northward_m_s)
        .any(|value| !value.is_finite())
    {
        return Err(GridError::InvalidCoordinate);
    }
    let weight_sum = weights.into_iter().sum::<f64>();
    if (weight_sum - 1.0).abs() > 1.0e-12 || weights.into_iter().any(|weight| weight < 0.0) {
        return Err(GridError::InvalidWeights);
    }
    Ok(())
}

fn interpolate_spherical_vector_in_bases(
    source_bases: &[SphericalBasis; 4],
    query_basis: SphericalBasis,
    eastward_m_s: [f64; 4],
    northward_m_s: [f64; 4],
    weights: [f64; 4],
) -> (f64, f64) {
    let mut cartesian = [0.0; 3];
    for index in 0..4 {
        let vector = source_bases[index].embed(eastward_m_s[index], northward_m_s[index]);
        for component in 0..3 {
            cartesian[component] += weights[index] * vector[component];
        }
    }
    query_basis.project(cartesian)
}

fn dot(left: [f64; 3], right: [f64; 3]) -> f64 {
    left[0].mul_add(right[0], left[1].mul_add(right[1], left[2] * right[2]))
}

/// Selects one domain for both time endpoints of a query.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DomainSelector {
    /// Candidate geometries in descending preference order.
    pub domains: Vec<DomainGeometry>,
}

impl DomainSelector {
    /// Selects the first preferred domain that safely covers the point.
    pub fn select(
        &self,
        longitude_degrees: f64,
        latitude_degrees: f64,
    ) -> Result<DomainId, GridError> {
        let mut saw_polar_singularity = false;
        for geometry in &self.domains {
            let grid = RegularLatLonGrid::new(geometry.clone())?;
            match grid.locate_cell(longitude_degrees, latitude_degrees) {
                Ok(_) => return Ok(geometry.domain.clone()),
                Err(GridError::PolarSingularity) => saw_polar_singularity = true,
                Err(GridError::OutOfDomain) => {}
                Err(error) => return Err(error),
            }
        }
        if saw_polar_singularity {
            Err(GridError::PolarSingularity)
        } else {
            Err(GridError::OutOfDomain)
        }
    }
}

/// Explicit future extension point for grid-to-grid transformation plans.
pub trait RegridPlan: Send + Sync {
    /// Applies a prevalidated regridding plan without changing query semantics.
    fn execute(&self, source: &[f64], destination: &mut [f64]) -> Result<(), GridError>;
}

/// Horizontal geometry or location failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum GridError {
    /// Geographic point is outside safe interpolation bounds.
    OutOfDomain,
    /// Exact geographic poles have no unique local east direction.
    PolarSingularity,
    /// A coordinate is NaN, infinite, or otherwise malformed.
    InvalidCoordinate,
    /// Interpolation weights are non-finite, negative, or do not sum to one.
    InvalidWeights,
    /// Grid metadata is invalid.
    InvalidGeometry(String),
    /// Stable integer cell encoding overflowed.
    IndexOverflow,
    /// Finite vector arithmetic failed.
    NumericalFailure,
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn global_grid() -> RegularLatLonGrid {
        RegularLatLonGrid::new(DomainGeometry {
            domain: DomainId("global".into()),
            longitude_origin_degrees: 0.0,
            latitude_origin_degrees: -89.0,
            longitude_spacing_degrees: 1.0,
            latitude_spacing_degrees: 1.0,
            nx: 360,
            ny: 179,
            periodic_longitude: true,
            halo_cells: 0,
        })
        .unwrap()
    }

    #[test]
    fn periodic_longitude_is_representation_invariant_at_date_line() {
        let grid = global_grid();
        let west = grid.horizontal_weights(-0.25, 10.25).unwrap();
        let east = grid.horizontal_weights(359.75, 10.25).unwrap();
        assert_eq!(west, east);
        assert_eq!(west.points[0].x, 359);
        assert_eq!(west.points[1].x, 0);
    }

    #[test]
    fn affine_scalar_field_is_exact_under_bilinear_weights() {
        let grid = global_grid();
        let weights = grid.horizontal_weights(12.25, 20.75).unwrap();
        let values = weights
            .points
            .map(|point| 2.0 * point.x as f64 + 3.0 * point.y as f64 + 5.0);
        let interpolated = values
            .into_iter()
            .zip(weights.weights)
            .map(|(value, weight)| value * weight)
            .sum::<f64>();
        let fractional_y = 20.75 - (-89.0);
        let expected = 2.0 * 12.25 + 3.0 * fractional_y + 5.0;
        assert!((interpolated - expected).abs() < 1.0e-12);
    }

    #[test]
    fn exact_high_safe_boundary_uses_the_interior_cell() {
        let grid = RegularLatLonGrid::new(DomainGeometry {
            domain: DomainId("limited".into()),
            longitude_origin_degrees: 0.0,
            latitude_origin_degrees: 5.0,
            longitude_spacing_degrees: 1.0,
            latitude_spacing_degrees: -1.0,
            nx: 6,
            ny: 6,
            periodic_longitude: false,
            halo_cells: 1,
        })
        .unwrap();

        let weights = grid.horizontal_weights(4.0, 1.0).unwrap();
        assert_eq!(
            weights.points,
            [
                GridPoint { x: 3, y: 3 },
                GridPoint { x: 4, y: 3 },
                GridPoint { x: 3, y: 4 },
                GridPoint { x: 4, y: 4 },
            ]
        );
        assert_eq!(weights.weights, [0.0, 0.0, 0.0, 1.0]);
        assert_eq!(
            grid.horizontal_weights(4.000_001, 1.0),
            Err(GridError::OutOfDomain)
        );
        assert_eq!(
            grid.horizontal_weights(4.0, 0.999_999),
            Err(GridError::OutOfDomain)
        );
    }

    #[test]
    fn exact_poles_are_typed_but_near_poles_are_queryable() {
        let grid = global_grid();
        assert_eq!(
            grid.horizontal_weights(0.0, 90.0),
            Err(GridError::PolarSingularity)
        );
        assert!(grid.horizontal_weights(0.0, 88.9).is_ok());
    }

    #[test]
    fn spherical_vector_does_not_depend_on_longitude_wrap() {
        let source_lon = [179.0, -179.0, 179.0, -179.0];
        let source_lat = [10.0, 10.0, 11.0, 11.0];
        let eastward = [10.0; 4];
        let northward = [2.0; 4];
        let weights = [0.25; 4];
        let left = interpolate_spherical_vector(
            source_lon, source_lat, eastward, northward, weights, -180.0, 10.5,
        )
        .unwrap();
        let right = interpolate_spherical_vector(
            source_lon, source_lat, eastward, northward, weights, 180.0, 10.5,
        )
        .unwrap();
        assert!((left.0 - right.0).abs() < 1.0e-12);
        assert!((left.1 - right.1).abs() < 1.0e-12);
    }

    #[test]
    fn precomputed_spherical_bases_preserve_vector_interpolation_exactly() {
        let source_lon = [179.0, -179.0, 179.0, -179.0];
        let source_lat = [10.0, 10.0, 11.0, 11.0];
        let eastward = [10.0, -2.0, 4.0, 8.0];
        let northward = [2.0, 3.0, -1.0, 5.0];
        let weights = [0.21, 0.29, 0.24, 0.26];
        let expected = interpolate_spherical_vector(
            source_lon, source_lat, eastward, northward, weights, 180.0, 10.5,
        )
        .unwrap();
        let source_bases = [
            SphericalBasis::at_degrees(source_lon[0], source_lat[0]).unwrap(),
            SphericalBasis::at_degrees(source_lon[1], source_lat[1]).unwrap(),
            SphericalBasis::at_degrees(source_lon[2], source_lat[2]).unwrap(),
            SphericalBasis::at_degrees(source_lon[3], source_lat[3]).unwrap(),
        ];
        let query_basis = spherical_vector_query_basis(180.0, 10.5).unwrap();

        assert_eq!(
            interpolate_spherical_vector_with_bases(
                &source_bases,
                query_basis,
                eastward,
                northward,
                weights,
            )
            .unwrap(),
            expected
        );
    }
}
