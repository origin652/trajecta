//! # Contract: horizontal grids and spherical geometry
//!
//! v0 executes on regular latitude-longitude grids. Location and weights must
//! handle global periodic longitude, the date line, poles, latitude bounds,
//! and a one-cell interpolation halo. Wind interpolation uses three-dimensional
//! tangent vectors rather than independently interpolating longitude/latitude
//! components.

use trajecta_case::model::meteorology::DomainId;

/// Integer horizontal grid point.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct GridPoint {
    /// Zero-based x or longitude index.
    pub x: usize,
    /// Zero-based y or latitude index.
    pub y: usize,
}

/// Stable horizontal cell identifier.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct CellId(pub u64);

/// Horizontal interpolation support and weights.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HorizontalWeights {
    /// Four surrounding points in deterministic order.
    pub points: [GridPoint; 4],
    /// Four scalar weights in the same order.
    pub weights: [f64; 4],
    /// Validity mask used by triangle-aware policies.
    pub valid: [bool; 4],
}

/// Horizontal domain geometry and safe interpolation bounds.
#[derive(Clone, Debug, PartialEq)]
pub struct DomainGeometry {
    /// Stable domain identifier.
    pub domain: DomainId,
    /// Western grid longitude in degrees east.
    pub longitude_origin_degrees: f64,
    /// Southern or first grid latitude in degrees north.
    pub latitude_origin_degrees: f64,
    /// Positive longitude spacing in degrees.
    pub longitude_spacing_degrees: f64,
    /// Signed latitude spacing in degrees.
    pub latitude_spacing_degrees: f64,
    /// Number of longitude points.
    pub nx: usize,
    /// Number of latitude points.
    pub ny: usize,
    /// Whether longitude is periodic.
    pub periodic_longitude: bool,
    /// Required safe halo in grid cells.
    pub halo_cells: usize,
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

/// v0 regular latitude-longitude grid backend.
#[derive(Clone, Debug, PartialEq)]
pub struct RegularLatLonGrid {
    geometry: DomainGeometry,
}

impl RegularLatLonGrid {
    /// Creates a backend from validated geometry.
    #[must_use]
    pub const fn new(geometry: DomainGeometry) -> Self {
        Self { geometry }
    }
}

impl GridBackend for RegularLatLonGrid {
    fn geometry(&self) -> &DomainGeometry {
        &self.geometry
    }

    fn locate_cell(
        &self,
        _longitude_degrees: f64,
        _latitude_degrees: f64,
    ) -> Result<CellId, GridError> {
        Err(GridError::NotImplemented)
    }

    fn horizontal_weights(
        &self,
        _longitude_degrees: f64,
        _latitude_degrees: f64,
    ) -> Result<HorizontalWeights, GridError> {
        Err(GridError::NotImplemented)
    }
}

/// Local orthonormal east/north basis embedded in Earth-centered coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SphericalBasis {
    /// East unit vector.
    pub east: [f64; 3],
    /// North unit vector.
    pub north: [f64; 3],
}

/// Selects one domain for both time endpoints of a query.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DomainSelector {
    /// Candidate geometries in descending preference order.
    pub domains: Vec<DomainGeometry>,
}

impl DomainSelector {
    /// Selects one domain that safely covers the point at both time endpoints.
    pub fn select(
        &self,
        _longitude_degrees: f64,
        _latitude_degrees: f64,
    ) -> Result<DomainId, GridError> {
        Err(GridError::NotImplemented)
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
    /// Grid algorithms have not been implemented yet.
    NotImplemented,
    /// Geographic point is outside safe interpolation bounds.
    OutOfDomain,
    /// Grid metadata is invalid.
    InvalidGeometry(String),
}
