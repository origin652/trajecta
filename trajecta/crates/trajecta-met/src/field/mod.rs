//! # Contract: canonical meteorological fields
//!
//! Canonical identifiers have stable physical semantics independent of file
//! format or dataset naming. A profile maps exact source identities into this
//! registry; it may not redefine canonical dimensions, staggering, or
//! interpolation categories.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use trajecta_case::quantity::{Dimension, Unit};

use crate::vertical::VerticalStagger;

/// Built-in field identifier with stable physical meaning.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
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
    /// Air pressure at a full model level or query sample.
    AirPressure,
    /// Specific humidity.
    SpecificHumidity,
    /// Relative humidity.
    RelativeHumidity,
    /// Surface pressure.
    SurfacePressure,
    /// Surface geopotential.
    SurfaceGeopotential,
    /// Geometric height above mean sea level.
    GeometricHeight,
    /// Geometric terrain height above mean sea level.
    GeometricTerrainHeight,
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
    /// Eastward wind measured or diagnosed at 10 metres above ground.
    TenMetreEastwardWind,
    /// Northward wind measured or diagnosed at 10 metres above ground.
    TenMetreNorthwardWind,
    /// Air temperature measured or diagnosed at 2 metres above ground.
    TwoMetreAirTemperature,
    /// Specific humidity measured or diagnosed at 2 metres above ground.
    TwoMetreSpecificHumidity,
    /// Aerodynamic roughness length used by the surface-layer model.
    AerodynamicRoughnessLength,
    /// Eastward surface stress on the atmosphere.
    EastwardSurfaceStress,
    /// Northward surface stress on the atmosphere.
    NorthwardSurfaceStress,
    /// Surface sensible heat flux, positive upward from the surface.
    SensibleHeatFlux,
    /// Surface latent heat flux, positive upward from the surface.
    LatentHeatFlux,
    /// Surface friction velocity.
    FrictionVelocity,
    /// Monin-Obukhov length.
    MoninObukhovLength,
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

impl CanonicalField {
    /// Every built-in field in stable declaration order.
    pub const ALL: [Self; 34] = [
        Self::EastwardWind,
        Self::NorthwardWind,
        Self::GeometricVerticalVelocity,
        Self::PressureVerticalVelocity,
        Self::HybridVerticalVelocity,
        Self::AirTemperature,
        Self::AirPressure,
        Self::SpecificHumidity,
        Self::RelativeHumidity,
        Self::SurfacePressure,
        Self::SurfaceGeopotential,
        Self::GeometricHeight,
        Self::GeometricTerrainHeight,
        Self::GeopotentialHeight,
        Self::AirDensity,
        Self::PotentialVorticity,
        Self::PrecipitationRate,
        Self::BoundaryLayerHeight,
        Self::TenMetreEastwardWind,
        Self::TenMetreNorthwardWind,
        Self::TwoMetreAirTemperature,
        Self::TwoMetreSpecificHumidity,
        Self::AerodynamicRoughnessLength,
        Self::EastwardSurfaceStress,
        Self::NorthwardSurfaceStress,
        Self::SensibleHeatFlux,
        Self::LatentHeatFlux,
        Self::FrictionVelocity,
        Self::MoninObukhovLength,
        Self::ConvectiveVelocityScale,
        Self::LandSeaMask,
        Self::CloudLiquidWater,
        Self::CloudIceWater,
        Self::OzoneMassMixingRatio,
    ];

    /// Returns the frozen physical dimension independently of unit spelling.
    #[must_use]
    pub const fn dimension(self) -> Dimension {
        use CanonicalField as F;

        match self {
            F::EastwardWind
            | F::NorthwardWind
            | F::GeometricVerticalVelocity
            | F::TenMetreEastwardWind
            | F::TenMetreNorthwardWind
            | F::FrictionVelocity
            | F::ConvectiveVelocityScale => Dimension::VELOCITY,
            F::PressureVerticalVelocity => Dimension::PRESSURE_TENDENCY,
            F::HybridVerticalVelocity => Dimension::new(0, 0, -1, 0),
            F::AirTemperature | F::TwoMetreAirTemperature => Dimension::TEMPERATURE,
            F::AirPressure
            | F::SurfacePressure
            | F::EastwardSurfaceStress
            | F::NorthwardSurfaceStress => Dimension::PRESSURE,
            F::SpecificHumidity
            | F::RelativeHumidity
            | F::TwoMetreSpecificHumidity
            | F::LandSeaMask
            | F::CloudLiquidWater
            | F::CloudIceWater
            | F::OzoneMassMixingRatio => Dimension::DIMENSIONLESS,
            F::SurfaceGeopotential => Dimension::GEOPOTENTIAL,
            F::GeometricHeight
            | F::GeometricTerrainHeight
            | F::GeopotentialHeight
            | F::BoundaryLayerHeight
            | F::AerodynamicRoughnessLength
            | F::MoninObukhovLength => Dimension::LENGTH,
            F::AirDensity => Dimension::DENSITY,
            F::PotentialVorticity => Dimension::new(-1, 2, -1, 1),
            F::PrecipitationRate => Dimension::new(1, -2, -1, 0),
            F::SensibleHeatFlux | F::LatentHeatFlux => Dimension::ENERGY_FLUX,
        }
    }

    /// Returns the frozen M3 physical and numerical semantics of this field.
    #[must_use]
    pub const fn semantics(self) -> CanonicalFieldSemantics {
        use CanonicalField as F;

        let mut semantics = match self {
            F::EastwardWind | F::NorthwardWind => CanonicalFieldSemantics::full_level(
                "m/s",
                Capability::Transport,
                InterpolationKind::SphericalVector,
            ),
            F::GeometricVerticalVelocity => CanonicalFieldSemantics::full_level(
                "m/s",
                Capability::Transport,
                InterpolationKind::MaskedTriangle,
            ),
            F::PressureVerticalVelocity => CanonicalFieldSemantics::full_level(
                "Pa/s",
                Capability::Transport,
                InterpolationKind::MaskedTriangle,
            ),
            F::HybridVerticalVelocity => CanonicalFieldSemantics::full_level(
                "s-1",
                Capability::Transport,
                InterpolationKind::MaskedTriangle,
            ),
            F::AirTemperature => CanonicalFieldSemantics::full_level(
                "K",
                Capability::Transport,
                InterpolationKind::MaskedTriangle,
            ),
            F::AirPressure => CanonicalFieldSemantics::full_level(
                "Pa",
                Capability::Transport,
                InterpolationKind::MaskedTriangle,
            ),
            F::SpecificHumidity => CanonicalFieldSemantics::full_level(
                "1",
                Capability::Transport,
                InterpolationKind::MaskedTriangle,
            ),
            F::RelativeHumidity => CanonicalFieldSemantics::full_level(
                "1",
                Capability::Diagnostics,
                InterpolationKind::MaskedTriangle,
            ),
            F::SurfacePressure => CanonicalFieldSemantics::horizontal(
                "Pa",
                Capability::Transport,
                InterpolationKind::ScalarLinear,
            ),
            F::SurfaceGeopotential => CanonicalFieldSemantics::horizontal(
                "m2/s2",
                Capability::Transport,
                InterpolationKind::ScalarLinear,
            ),
            F::GeometricHeight | F::GeopotentialHeight => CanonicalFieldSemantics::full_level(
                "m",
                Capability::Transport,
                InterpolationKind::MaskedTriangle,
            ),
            F::GeometricTerrainHeight => CanonicalFieldSemantics::horizontal(
                "m",
                Capability::NearSurfaceTransport,
                InterpolationKind::ScalarLinear,
            ),
            F::AirDensity => CanonicalFieldSemantics::full_level(
                "kg/m3",
                Capability::Transport,
                InterpolationKind::MaskedTriangle,
            ),
            F::PotentialVorticity => CanonicalFieldSemantics::full_level(
                "K m2 kg-1 s-1",
                Capability::Diagnostics,
                InterpolationKind::MaskedTriangle,
            ),
            F::PrecipitationRate => CanonicalFieldSemantics::horizontal(
                "kg/m2/s",
                Capability::WetDeposition,
                InterpolationKind::ScalarLinear,
            ),
            F::BoundaryLayerHeight => CanonicalFieldSemantics::horizontal(
                "m",
                Capability::NearSurfaceTransport,
                InterpolationKind::ScalarLinear,
            ),
            F::TenMetreEastwardWind | F::TenMetreNorthwardWind => {
                CanonicalFieldSemantics::horizontal(
                    "m/s",
                    Capability::NearSurfaceTransport,
                    InterpolationKind::SphericalVector,
                )
            }
            F::TwoMetreAirTemperature => CanonicalFieldSemantics::horizontal(
                "K",
                Capability::NearSurfaceTransport,
                InterpolationKind::ScalarLinear,
            ),
            F::TwoMetreSpecificHumidity => CanonicalFieldSemantics::horizontal(
                "1",
                Capability::NearSurfaceTransport,
                InterpolationKind::ScalarLinear,
            ),
            F::AerodynamicRoughnessLength => CanonicalFieldSemantics::horizontal(
                "m",
                Capability::NearSurfaceTransport,
                InterpolationKind::ScalarLinear,
            ),
            F::EastwardSurfaceStress | F::NorthwardSurfaceStress => {
                CanonicalFieldSemantics::horizontal(
                    "Pa",
                    Capability::NearSurfaceTransport,
                    InterpolationKind::ScalarLinear,
                )
            }
            F::SensibleHeatFlux | F::LatentHeatFlux => CanonicalFieldSemantics::horizontal(
                "kg/s3",
                Capability::NearSurfaceTransport,
                InterpolationKind::ScalarLinear,
            ),
            F::FrictionVelocity => CanonicalFieldSemantics::horizontal(
                "m/s",
                Capability::NearSurfaceTransport,
                InterpolationKind::ScalarLinear,
            ),
            F::MoninObukhovLength => CanonicalFieldSemantics::horizontal(
                "m",
                Capability::NearSurfaceTransport,
                InterpolationKind::ScalarLinear,
            ),
            F::ConvectiveVelocityScale => CanonicalFieldSemantics::horizontal(
                "m/s",
                Capability::Convection,
                InterpolationKind::ScalarLinear,
            ),
            F::LandSeaMask => CanonicalFieldSemantics::horizontal(
                "1",
                Capability::DryDeposition,
                InterpolationKind::Categorical,
            ),
            F::CloudLiquidWater | F::CloudIceWater => CanonicalFieldSemantics::full_level(
                "1",
                Capability::WetDeposition,
                InterpolationKind::MaskedTriangle,
            ),
            F::OzoneMassMixingRatio => CanonicalFieldSemantics::full_level(
                "1",
                Capability::Diagnostics,
                InterpolationKind::MaskedTriangle,
            ),
        };
        semantics.dimension = self.dimension();
        semantics
    }
}

/// Frozen canonical unit, layout, staggering, capability, and interpolation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CanonicalFieldSemantics {
    /// Canonical unit spelling accepted by the Profile type checker.
    pub unit: &'static str,
    /// Fundamental SI dimension independent of unit spelling.
    pub dimension: Dimension,
    /// Canonical source/output array shape.
    pub shape: FieldShape,
    /// Native vertical staggering, if any.
    pub vertical_stagger: Option<VerticalStagger>,
    /// Capability required to publish this field.
    pub required_capability: Capability,
    /// Horizontal interpolation category.
    pub interpolation: InterpolationKind,
}

impl CanonicalFieldSemantics {
    const fn full_level(
        unit: &'static str,
        required_capability: Capability,
        interpolation: InterpolationKind,
    ) -> Self {
        Self {
            unit,
            dimension: Dimension::DIMENSIONLESS,
            shape: FieldShape::Full3D,
            vertical_stagger: Some(VerticalStagger::Full),
            required_capability,
            interpolation,
        }
    }

    const fn horizontal(
        unit: &'static str,
        required_capability: Capability,
        interpolation: InterpolationKind,
    ) -> Self {
        Self {
            unit,
            dimension: Dimension::DIMENSIONLESS,
            shape: FieldShape::Horizontal2D,
            vertical_stagger: None,
            required_capability,
            interpolation,
        }
    }
}

/// Namespaced non-canonical field identifier.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
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
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
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
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FieldQuality {
    /// Decoded directly from a locked source.
    Source,
    /// Deterministically derived from source fields.
    Derived,
    /// Produced by an explicitly allowed fallback or estimate.
    Estimated,
}

/// Stable interpolation category fixed by field semantics.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
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
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    /// Three-dimensional transport winds and vertical velocity.
    Transport = 0,
    /// Surface and near-surface fields required by complete transport queries.
    NearSurfaceTransport = 1,
    /// Planetary boundary-layer diagnostics.
    PlanetaryBoundaryLayer = 2,
    /// Convective diagnostics.
    Convection = 3,
    /// Wet-deposition inputs.
    WetDeposition = 4,
    /// Dry-deposition inputs.
    DryDeposition = 5,
    /// Dry-air mass and boundary-flux inputs.
    DomainFill = 6,
    /// Optional scientific diagnostics.
    Diagnostics = 7,
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

    /// Returns whether no capabilities are enabled.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// Returns a set containing `self` and one additional capability.
    #[must_use]
    pub const fn with(mut self, capability: Capability) -> Self {
        self.0 |= 1_u64 << capability as u32;
        self
    }

    /// Iterates enabled capabilities in stable enum order.
    pub fn iter(self) -> impl Iterator<Item = Capability> {
        [
            Capability::Transport,
            Capability::NearSurfaceTransport,
            Capability::PlanetaryBoundaryLayer,
            Capability::Convection,
            Capability::WetDeposition,
            Capability::DryDeposition,
            Capability::DomainFill,
            Capability::Diagnostics,
        ]
        .into_iter()
        .filter(move |capability| self.contains(*capability))
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
    /// Runtime capability required to publish this field.
    pub required_capability: Capability,
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

    /// Builds the complete frozen canonical registry with coherent SI units.
    ///
    /// Source/derived/estimated quality remains a runtime provenance property;
    /// registry entries use `Source` as the non-estimated planning baseline.
    pub fn canonical() -> Result<Self, FieldRegistryError> {
        let mut registry = Self::new();
        for field in CanonicalField::ALL {
            let semantics = field.semantics();
            let unit = Unit::new(semantics.unit, semantics.dimension, 1.0, 0.0)
                .map_err(|_| FieldRegistryError::InvalidCanonicalUnit(field))?;
            registry.register(FieldDescriptor {
                key: FieldKey::Canonical(field),
                unit,
                shape: semantics.shape,
                vertical_stagger: semantics.vertical_stagger,
                quality: FieldQuality::Source,
                required_capability: semantics.required_capability,
                interpolation: semantics.interpolation,
            })?;
        }
        Ok(registry)
    }

    /// Registers one unique field key.
    pub fn register(&mut self, descriptor: FieldDescriptor) -> Result<(), FieldRegistryError> {
        if self.descriptors.contains_key(&descriptor.key) {
            return Err(FieldRegistryError::DuplicateField(descriptor.key));
        }
        if let FieldKey::Canonical(field) = &descriptor.key {
            let semantics = field.semantics();
            if descriptor.unit.symbol() != semantics.unit
                || descriptor.unit.dimension() != semantics.dimension
                || descriptor.shape != semantics.shape
                || descriptor.vertical_stagger != semantics.vertical_stagger
                || descriptor.required_capability != semantics.required_capability
                || descriptor.interpolation != semantics.interpolation
            {
                return Err(FieldRegistryError::CanonicalContractMismatch(*field));
            }
        }
        self.descriptors.insert(descriptor.key.clone(), descriptor);
        Ok(())
    }

    /// Returns an exact descriptor.
    #[must_use]
    pub fn get(&self, key: &FieldKey) -> Option<&FieldDescriptor> {
        self.descriptors.get(key)
    }

    /// Returns the descriptor count.
    #[must_use]
    pub fn len(&self) -> usize {
        self.descriptors.len()
    }

    /// Returns whether no descriptors are registered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.descriptors.is_empty()
    }
}

/// Field-registry construction failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FieldRegistryError {
    /// A field key was registered more than once.
    DuplicateField(FieldKey),
    /// A canonical descriptor attempted to redefine frozen field semantics.
    CanonicalContractMismatch(CanonicalField),
    /// A frozen canonical unit definition is internally invalid.
    InvalidCanonicalUnit(CanonicalField),
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn canonical_registry_freezes_every_unit_dimension() {
        let registry = FieldRegistry::canonical().unwrap();
        assert_eq!(registry.len(), CanonicalField::ALL.len());
        for field in CanonicalField::ALL {
            let descriptor = registry.get(&FieldKey::Canonical(field)).unwrap();
            assert_eq!(descriptor.unit.symbol(), field.semantics().unit);
            assert_eq!(descriptor.unit.dimension(), field.dimension());
        }
        assert_eq!(CanonicalField::AirDensity.dimension(), Dimension::DENSITY);
        assert_eq!(
            CanonicalField::SurfaceGeopotential.dimension(),
            Dimension::GEOPOTENTIAL
        );
        assert_eq!(
            CanonicalField::SensibleHeatFlux.dimension(),
            Dimension::ENERGY_FLUX
        );
    }

    #[test]
    fn canonical_registration_rejects_correct_symbol_with_wrong_dimension() {
        let field = CanonicalField::AirDensity;
        let semantics = field.semantics();
        let mut registry = FieldRegistry::new();
        let error = registry
            .register(FieldDescriptor {
                key: FieldKey::Canonical(field),
                unit: Unit::new(semantics.unit, Dimension::DIMENSIONLESS, 1.0, 0.0).unwrap(),
                shape: semantics.shape,
                vertical_stagger: semantics.vertical_stagger,
                quality: FieldQuality::Source,
                required_capability: semantics.required_capability,
                interpolation: semantics.interpolation,
            })
            .unwrap_err();
        assert_eq!(error, FieldRegistryError::CanonicalContractMismatch(field));
    }
}
