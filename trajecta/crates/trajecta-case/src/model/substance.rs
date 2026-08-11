//! # Contract: strongly typed substances
//!
//! A substance kind fixes its accepted physical properties. Values carrying a
//! dimension require an explicit unit at the Case boundary and are stored in
//! SI units after deserialization.

use std::fmt;

use serde::de;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::quantity::{
    Dimension, Length, Quantity, QuantityError, QuantityInput, Temperature, Time, Unit,
};

/// Stable substance identifier used in particle state arrays and result rows.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct SubstanceId(pub String);

/// Substance category selected by a strongly typed Case entry.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubstanceKind {
    /// Atmospheric water vapour.
    WaterVapor,
    /// Gas with deposition or first-order loss properties.
    Gas,
    /// Size-resolved aerosol particle.
    Aerosol,
}

/// Supported aerosol shape for M6 deposition and settling.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AerosolShape {
    /// Spherical particle.
    Sphere,
}

/// Diameter distribution used for immutable birth sampling.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AerosolDiameterDistribution {
    /// Truncated log-normal distribution weighted by aerosol mass.
    TruncatedLognormalMassBasis,
}

/// Truncated log-normal aerosol diameter specification.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AerosolDiameterSpec {
    /// Frozen distribution family.
    pub distribution: AerosolDiameterDistribution,
    /// Geometric-mean physical diameter.
    pub geometric_mean: Quantity<crate::quantity::Length>,
    /// Dimensionless geometric standard deviation; validation requires `> 1`.
    pub geometric_standard_deviation: f64,
    /// Inclusive lower truncation diameter.
    pub minimum: Quantity<crate::quantity::Length>,
    /// Inclusive upper truncation diameter.
    pub maximum: Quantity<crate::quantity::Length>,
}

/// Temperature-dependent OH reaction coefficient.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OhReactionSpec {
    /// Pre-exponential coefficient in cubic metres per molecule per second.
    pub pre_exponential: OhRateCoefficient,
    /// Dimensionless temperature exponent.
    pub temperature_exponent: f64,
    /// Activation temperature in kelvin.
    pub activation_temperature: Quantity<Temperature>,
}

/// Strongly typed substance declaration.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SubstanceSpec {
    /// Water-vapour tracer; no gas or aerosol properties are accepted.
    WaterVapor {
        /// Stable identifier.
        id: SubstanceId,
        /// Human-readable name.
        display_name: String,
    },
    /// Gas with required transport properties and optional loss processes.
    Gas {
        /// Stable identifier.
        id: SubstanceId,
        /// Human-readable name.
        display_name: String,
        /// Molecular mass in kilograms per mole.
        molar_mass: MolarMass,
        /// Henry coefficient in moles per cubic metre per pascal.
        henry_constant: HenryConstant,
        /// Dimensionless surface reactivity used by resistance models.
        surface_reactivity: f64,
        /// Optional first-order half-life.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        half_life: Option<Quantity<Time>>,
        /// Optional temperature-dependent OH reaction parameters.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        oh_reaction: Option<OhReactionSpec>,
    },
    /// Aerosol with one immutable diameter sampled at particle birth.
    Aerosol {
        /// Stable identifier.
        id: SubstanceId,
        /// Human-readable name.
        display_name: String,
        /// Material density in kilograms per cubic metre.
        material_density: MaterialDensity,
        /// Frozen particle shape.
        shape: AerosolShape,
        /// Birth-diameter distribution.
        diameter: AerosolDiameterSpec,
    },
}

impl SubstanceSpec {
    /// Returns the stable identifier shared by all substance kinds.
    #[must_use]
    pub const fn id(&self) -> &SubstanceId {
        match self {
            Self::WaterVapor { id, .. } | Self::Gas { id, .. } | Self::Aerosol { id, .. } => id,
        }
    }

    /// Returns the display name shared by all substance kinds.
    #[must_use]
    pub fn display_name(&self) -> &str {
        match self {
            Self::WaterVapor { display_name, .. }
            | Self::Gas { display_name, .. }
            | Self::Aerosol { display_name, .. } => display_name,
        }
    }

    /// Returns the selected substance category.
    #[must_use]
    pub const fn kind(&self) -> SubstanceKind {
        match self {
            Self::WaterVapor { .. } => SubstanceKind::WaterVapor,
            Self::Gas { .. } => SubstanceKind::Gas,
            Self::Aerosol { .. } => SubstanceKind::Aerosol,
        }
    }

    /// Returns the resolved representation with every generic quantity using
    /// its canonical SI unit symbol.
    pub fn normalized_to_si(&self) -> Result<Self, QuantityError> {
        match self {
            Self::WaterVapor { id, display_name } => Ok(Self::WaterVapor {
                id: id.clone(),
                display_name: display_name.clone(),
            }),
            Self::Gas {
                id,
                display_name,
                molar_mass,
                henry_constant,
                surface_reactivity,
                half_life,
                oh_reaction,
            } => Ok(Self::Gas {
                id: id.clone(),
                display_name: display_name.clone(),
                molar_mass: *molar_mass,
                henry_constant: *henry_constant,
                surface_reactivity: *surface_reactivity,
                half_life: half_life.as_ref().map(si_seconds).transpose()?,
                oh_reaction: oh_reaction
                    .as_ref()
                    .map(|reaction| {
                        Ok(OhReactionSpec {
                            pre_exponential: reaction.pre_exponential,
                            temperature_exponent: reaction.temperature_exponent,
                            activation_temperature: si_kelvin(&reaction.activation_temperature)?,
                        })
                    })
                    .transpose()?,
            }),
            Self::Aerosol {
                id,
                display_name,
                material_density,
                shape,
                diameter,
            } => Ok(Self::Aerosol {
                id: id.clone(),
                display_name: display_name.clone(),
                material_density: *material_density,
                shape: *shape,
                diameter: AerosolDiameterSpec {
                    distribution: diameter.distribution,
                    geometric_mean: si_metres(&diameter.geometric_mean)?,
                    geometric_standard_deviation: diameter.geometric_standard_deviation,
                    minimum: si_metres(&diameter.minimum)?,
                    maximum: si_metres(&diameter.maximum)?,
                },
            }),
        }
    }
}

fn si_seconds(value: &Quantity<Time>) -> Result<Quantity<Time>, QuantityError> {
    Quantity::from_si(value.value_si(), Unit::new("s", Dimension::Time, 1.0, 0.0)?)
}

fn si_kelvin(value: &Quantity<Temperature>) -> Result<Quantity<Temperature>, QuantityError> {
    Quantity::from_si(
        value.value_si(),
        Unit::new("K", Dimension::Temperature, 1.0, 0.0)?,
    )
}

fn si_metres(value: &Quantity<Length>) -> Result<Quantity<Length>, QuantityError> {
    Quantity::from_si(
        value.value_si(),
        Unit::new("m", Dimension::Length, 1.0, 0.0)?,
    )
}

macro_rules! fixed_quantity {
    ($name:ident, $doc:literal, $canonical:literal, {$($unit:literal => $scale:expr),+ $(,)?}) => {
        #[doc = $doc]
        #[derive(Clone, Copy, Debug, PartialEq)]
        pub struct $name(f64);

        impl $name {
            /// Creates the value from its canonical SI representation.
            pub fn from_si(value: f64) -> Result<Self, FixedQuantityError> {
                if value.is_finite() {
                    Ok(Self(value))
                } else {
                    Err(FixedQuantityError::NonFinite)
                }
            }

            /// Returns the canonical SI value.
            #[must_use]
            pub const fn value_si(self) -> f64 {
                self.0
            }
        }

        impl Serialize for $name {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: Serializer,
            {
                QuantityInput::object(self.0, $canonical).serialize(serializer)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                let input = QuantityInput::deserialize(deserializer)?;
                let (value, unit) = input.split_value_unit().map_err(de::Error::custom)?;
                let scale = match unit.as_str() {
                    $($unit => $scale,)+
                    other => return Err(de::Error::custom(
                        FixedQuantityError::UnsupportedUnit {
                            quantity: stringify!($name),
                            unit: other.to_owned(),
                        }
                    )),
                };
                Self::from_si(value * scale).map_err(de::Error::custom)
            }
        }
    };
}

fixed_quantity!(
    MolarMass,
    "Molar mass stored in kilograms per mole.",
    "kg/mol",
    {"kg/mol" => 1.0, "g/mol" => 1.0e-3}
);
fixed_quantity!(
    HenryConstant,
    "Henry coefficient stored in moles per cubic metre per pascal.",
    "mol/(m3 Pa)",
    {"mol/(m3 Pa)" => 1.0}
);
fixed_quantity!(
    OhRateCoefficient,
    "OH rate coefficient stored in cubic metres per molecule per second.",
    "m3/(molecule s)",
    {"m3/(molecule s)" => 1.0, "cm3/(molecule s)" => 1.0e-6}
);
fixed_quantity!(
    MaterialDensity,
    "Material density stored in kilograms per cubic metre.",
    "kg/m3",
    {"kg/m3" => 1.0, "g/cm3" => 1.0e3}
);

/// Failure to decode a fixed-dimension substance property.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FixedQuantityError {
    /// The converted value is NaN or infinite.
    NonFinite,
    /// The supplied unit is not part of the property contract.
    UnsupportedUnit {
        /// Property type.
        quantity: &'static str,
        /// Rejected unit spelling.
        unit: String,
    },
}

impl fmt::Display for FixedQuantityError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NonFinite => formatter.write_str("quantity value must be finite"),
            Self::UnsupportedUnit { quantity, unit } => {
                write!(formatter, "unsupported {quantity} unit '{unit}'")
            }
        }
    }
}

impl std::error::Error for FixedQuantityError {}
