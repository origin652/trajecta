//! # Contract: canonical meteorological fields
//!
//! Canonical identifiers have stable physical semantics independent of file
//! format or dataset naming. A profile maps exact source identities into this
//! registry; it may not redefine canonical dimensions, staggering, or
//! interpolation categories.

use std::collections::BTreeMap;

use trajecta_case::quantity::Unit;

use crate::vertical::VerticalStagger;

/// Built-in field identifier with stable physical meaning.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[non_exhaustive]
pub enum CanonicalField {
    /// Eastward wind component in the local tangent basis.
    EastwardWind,
    /// Northward wind component in the local tangent basis.
    NorthwardWind,
    /// Geometric vertical velocity, positive upward.
    GeometricVerticalVelocity,
    /// Pressure vertical velocity omega.
    PressureVerticalVelocity,
    /// Native hybrid-coordinate vertical velocity.
    HybridVerticalVelocity,
    /// Air temperature.
    AirTemperature,
    /// Specific humidity.
    SpecificHumidity,
    /// Relative humidity.
    RelativeHumidity,
    /// Surface pressure.
    SurfacePressure,
    /// Surface geopotential.
    SurfaceGeopotential,
    /// Geopotential height.
    GeopotentialHeight,
    /// Dry or moist air density as declared by the descriptor.
    AirDensity,
    /// Potential vorticity.
    PotentialVorticity,
    /// Precipitation rate after de-accumulation.
    PrecipitationRate,
    /// Planetary boundary-layer height.
    BoundaryLayerHeight,
    /// Surface friction velocity.
    FrictionVelocity,
    /// Convective velocity scale.
    ConvectiveVelocityScale,
    /// Land-sea mask or land fraction.
    LandSeaMask,
    /// Cloud liquid water content.
    CloudLiquidWater,
    /// Cloud ice water content.
    CloudIceWater,
    /// Ozone mass mixing ratio.
    OzoneMassMixingRatio,
}

/// Namespaced non-canonical field identifier.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ExtensionFieldId {
    /// Owning namespace, such as an organization or plugin name.
    pub namespace: String,
    /// Identifier unique within the namespace.
    pub name: String,
}

/// Canonical or explicitly namespaced field key.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum FieldKey {
    /// Stable built-in field.
    Canonical(CanonicalField),
    /// Extension field that cannot shadow a canonical identifier.
    Extension(ExtensionFieldId),
}

/// Array shape and horizontal/vertical support of a field.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum FieldShape {
    /// One value per logical frame.
    Scalar,
    /// One value per horizontal grid point.
    Horizontal2D,
    /// One value per full model level and horizontal grid point.
    Full3D,
    /// One value per interface level and horizontal grid point.
    Interface3D,
}

/// Provenance quality of a field value.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum FieldQuality {
    /// Decoded directly from a locked source.
    Source,
    /// Deterministically derived from source fields.
    Derived,
    /// Produced by an explicitly allowed fallback or estimate.
    Estimated,
}

/// Stable interpolation category fixed by field semantics.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum InterpolationKind {
    /// Continuous scalar interpolation.
    ScalarLinear,
    /// Categorical or nearest-neighbor interpolation.
    Categorical,
    /// Spherical tangent-vector interpolation.
    SphericalVector,
    /// Mask-aware valid-triangle interpolation.
    MaskedTriangle,
}

/// Runtime capability required by a Case or physics module.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Capability {
    /// Three-dimensional transport winds and vertical velocity.
    Transport,
    /// Planetary boundary-layer diagnostics.
    PlanetaryBoundaryLayer,
    /// Convective diagnostics.
    Convection,
    /// Wet-deposition inputs.
    WetDeposition,
    /// Dry-deposition inputs.
    DryDeposition,
    /// Dry-air mass and boundary-flux inputs.
    DomainFill,
    /// Optional scientific diagnostics.
    Diagnostics,
}

/// Compact deterministic set of meteorological capabilities.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct CapabilitySet(u64);

impl CapabilitySet {
    /// Creates an empty set.
    #[must_use]
    pub const fn new() -> Self {
        Self(0)
    }

    /// Adds a capability.
    pub fn insert(&mut self, capability: Capability) {
        self.0 |= 1_u64 << capability as u32;
    }

    /// Returns whether a capability is present.
    #[must_use]
    pub const fn contains(self, capability: Capability) -> bool {
        self.0 & (1_u64 << capability as u32) != 0
    }

    /// Returns whether every capability in `required` is present.
    #[must_use]
    pub const fn contains_all(self, required: Self) -> bool {
        self.0 & required.0 == required.0
    }
}

/// Immutable physical and numerical semantics of one field.
#[derive(Clone, Debug, PartialEq)]
pub struct FieldDescriptor {
    /// Field identity.
    pub key: FieldKey,
    /// Canonical SI-compatible unit.
    pub unit: Unit,
    /// Array shape.
    pub shape: FieldShape,
    /// Native vertical staggering.
    pub vertical_stagger: Option<VerticalStagger>,
    /// Expected provenance quality.
    pub quality: FieldQuality,
    /// Fixed interpolation category.
    pub interpolation: InterpolationKind,
}

/// Registry of canonical and extension descriptors.
#[derive(Clone, Debug, Default)]
pub struct FieldRegistry {
    descriptors: BTreeMap<FieldKey, FieldDescriptor>,
}

impl FieldRegistry {
    /// Creates an empty registry.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            descriptors: BTreeMap::new(),
        }
    }

    /// Registers one unique field key.
    pub fn register(&mut self, descriptor: FieldDescriptor) -> Result<(), FieldRegistryError> {
        if self.descriptors.contains_key(&descriptor.key) {
            return Err(FieldRegistryError::DuplicateField(descriptor.key));
        }
        self.descriptors.insert(descriptor.key.clone(), descriptor);
        Ok(())
    }

    /// Returns an exact descriptor.
    #[must_use]
    pub fn get(&self, key: &FieldKey) -> Option<&FieldDescriptor> {
        self.descriptors.get(key)
    }
}

/// Field-registry construction failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FieldRegistryError {
    /// A field key was registered more than once.
    DuplicateField(FieldKey),
}
