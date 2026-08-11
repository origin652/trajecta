//! # Contract: physical quantities
//!
//! Scientific values carry an explicit unit and a compile-time dimension.
//! Inputs may use object or text syntax; resolved values are normalized to SI.
//! Bare numbers are never assigned an implicit unit.
//!
//! ## Exposed interface
//!
//! | Item | Role |
//! |---|---|
//! | [`Dimension`] | Runtime physical dimension |
//! | [`DimensionMarker`] | Compile-time dimension tag |
//! | [`Unit`] | Named affine conversion into SI |
//! | [`Quantity<D>`] | Typed SI-normalized quantity |
//! | [`QuantityInput`] | Unresolved object/text syntax |
//! | [`UnitRegistry`] | Exact symbol → unit lookup |
//! | [`QuantityError`] | Construction / parse failures |
//!
//! Deserialization of [`Unit`] always calls [`Unit::new`]; empty symbols, zero
//! scales, and non-finite affine parameters cannot bypass construction.

use std::collections::BTreeMap;
use std::fmt;
use std::marker::PhantomData;
use std::str::FromStr;

use serde::de::{self, Deserializer, MapAccess, Visitor};
use serde::ser::SerializeStruct;
use serde::{Deserialize, Serialize, Serializer};

/// Runtime physical dimension used by schemas and computation graphs.
///
/// Dimensions are reduced to SI base exponents so compound quantities such
/// as density, geopotential, and energy flux cannot be represented by an
/// unrelated placeholder category. The four exponents are ordered as mass,
/// length, time, and thermodynamic temperature.
///
/// Serde preserves the v0 string representation for the original named
/// dimensions. Other dimensions use the stable object representation
/// `{mass, length, time, temperature}`.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Dimension {
    /// Mass exponent.
    pub mass: i8,
    /// Length exponent.
    pub length: i8,
    /// Time exponent.
    pub time: i8,
    /// Thermodynamic-temperature exponent.
    pub temperature: i8,
}

impl Dimension {
    /// Dimensionless ratio or count.
    pub const DIMENSIONLESS: Self = Self::new(0, 0, 0, 0);
    /// Time.
    pub const TIME: Self = Self::new(0, 0, 1, 0);
    /// Length.
    pub const LENGTH: Self = Self::new(0, 1, 0, 0);
    /// Pressure.
    pub const PRESSURE: Self = Self::new(1, -1, -2, 0);
    /// Mass.
    pub const MASS: Self = Self::new(1, 0, 0, 0);
    /// Thermodynamic temperature.
    pub const TEMPERATURE: Self = Self::new(0, 0, 0, 1);
    /// Speed or velocity.
    pub const VELOCITY: Self = Self::new(0, 1, -1, 0);
    /// Pressure tendency such as omega (`Pa s-1`).
    pub const PRESSURE_TENDENCY: Self = Self::new(1, -1, -3, 0);
    /// Mass density (`kg m-3`).
    pub const DENSITY: Self = Self::new(1, -3, 0, 0);
    /// Geopotential (`m2 s-2`).
    pub const GEOPOTENTIAL: Self = Self::new(0, 2, -2, 0);
    /// Energy flux density (`W m-2`, equivalently `kg s-3`).
    pub const ENERGY_FLUX: Self = Self::new(1, 0, -3, 0);

    // Compatibility spellings retained for existing public callers and v0
    // code. New code should prefer the conventional upper-case constants.
    #[allow(non_upper_case_globals)]
    /// Compatibility alias for [`Self::DIMENSIONLESS`].
    pub const Dimensionless: Self = Self::DIMENSIONLESS;
    #[allow(non_upper_case_globals)]
    /// Compatibility alias for [`Self::TIME`].
    pub const Time: Self = Self::TIME;
    #[allow(non_upper_case_globals)]
    /// Compatibility alias for [`Self::LENGTH`].
    pub const Length: Self = Self::LENGTH;
    #[allow(non_upper_case_globals)]
    /// Compatibility alias for [`Self::PRESSURE`].
    pub const Pressure: Self = Self::PRESSURE;
    #[allow(non_upper_case_globals)]
    /// Compatibility alias for [`Self::MASS`].
    pub const Mass: Self = Self::MASS;
    #[allow(non_upper_case_globals)]
    /// Compatibility alias for [`Self::TEMPERATURE`].
    pub const Temperature: Self = Self::TEMPERATURE;
    #[allow(non_upper_case_globals)]
    /// Compatibility alias for [`Self::VELOCITY`].
    pub const Velocity: Self = Self::VELOCITY;

    /// Creates a dimension from SI base exponents.
    #[must_use]
    pub const fn new(mass: i8, length: i8, time: i8, temperature: i8) -> Self {
        Self {
            mass,
            length,
            time,
            temperature,
        }
    }

    /// Converts a named or compound dimension into the shared exponent form.
    #[must_use]
    pub const fn from_named(dimension: Self) -> Self {
        dimension
    }

    /// Multiplies dimensions, returning `None` on exponent overflow.
    #[must_use]
    pub fn checked_product(self, other: Self) -> Option<Self> {
        Some(Self {
            mass: self.mass.checked_add(other.mass)?,
            length: self.length.checked_add(other.length)?,
            time: self.time.checked_add(other.time)?,
            temperature: self.temperature.checked_add(other.temperature)?,
        })
    }

    /// Divides dimensions, returning `None` on exponent overflow.
    #[must_use]
    pub fn checked_quotient(self, other: Self) -> Option<Self> {
        Some(Self {
            mass: self.mass.checked_sub(other.mass)?,
            length: self.length.checked_sub(other.length)?,
            time: self.time.checked_sub(other.time)?,
            temperature: self.temperature.checked_sub(other.temperature)?,
        })
    }

    /// Raises a dimension to an integer power, returning `None` on overflow.
    #[must_use]
    pub fn checked_power(self, exponent: i8) -> Option<Self> {
        Some(Self {
            mass: self.mass.checked_mul(exponent)?,
            length: self.length.checked_mul(exponent)?,
            time: self.time.checked_mul(exponent)?,
            temperature: self.temperature.checked_mul(exponent)?,
        })
    }

    fn legacy_name(self) -> Option<&'static str> {
        if self == Self::DIMENSIONLESS {
            Some("dimensionless")
        } else if self == Self::TIME {
            Some("time")
        } else if self == Self::LENGTH {
            Some("length")
        } else if self == Self::PRESSURE {
            Some("pressure")
        } else if self == Self::MASS {
            Some("mass")
        } else if self == Self::TEMPERATURE {
            Some("temperature")
        } else if self == Self::VELOCITY {
            Some("velocity")
        } else {
            None
        }
    }
}

impl Serialize for Dimension {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        if let Some(name) = self.legacy_name() {
            return serializer.serialize_str(name);
        }
        let mut state = serializer.serialize_struct("Dimension", 4)?;
        state.serialize_field("mass", &self.mass)?;
        state.serialize_field("length", &self.length)?;
        state.serialize_field("time", &self.time)?;
        state.serialize_field("temperature", &self.temperature)?;
        state.end()
    }
}

impl<'de> Deserialize<'de> for Dimension {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct DimensionVisitor;

        impl<'de> Visitor<'de> for DimensionVisitor {
            type Value = Dimension;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter
                    .write_str("a named physical dimension or {mass, length, time, temperature}")
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                match value {
                    "dimensionless" => Ok(Dimension::DIMENSIONLESS),
                    "time" => Ok(Dimension::TIME),
                    "length" => Ok(Dimension::LENGTH),
                    "pressure" => Ok(Dimension::PRESSURE),
                    "mass" => Ok(Dimension::MASS),
                    "temperature" => Ok(Dimension::TEMPERATURE),
                    "velocity" => Ok(Dimension::VELOCITY),
                    other => Err(de::Error::unknown_variant(
                        other,
                        &[
                            "dimensionless",
                            "time",
                            "length",
                            "pressure",
                            "mass",
                            "temperature",
                            "velocity",
                        ],
                    )),
                }
            }

            fn visit_map<M>(self, mut map: M) -> Result<Self::Value, M::Error>
            where
                M: MapAccess<'de>,
            {
                let mut mass = None;
                let mut length = None;
                let mut time = None;
                let mut temperature = None;
                while let Some(key) = map.next_key::<String>()? {
                    let slot = match key.as_str() {
                        "mass" => &mut mass,
                        "length" => &mut length,
                        "time" => &mut time,
                        "temperature" => &mut temperature,
                        other => {
                            return Err(de::Error::unknown_field(
                                other,
                                &["mass", "length", "time", "temperature"],
                            ));
                        }
                    };
                    if slot.is_some() {
                        return Err(de::Error::duplicate_field(match key.as_str() {
                            "mass" => "mass",
                            "length" => "length",
                            "time" => "time",
                            _ => "temperature",
                        }));
                    }
                    *slot = Some(map.next_value::<i8>()?);
                }
                Ok(Dimension::new(
                    mass.ok_or_else(|| de::Error::missing_field("mass"))?,
                    length.ok_or_else(|| de::Error::missing_field("length"))?,
                    time.ok_or_else(|| de::Error::missing_field("time"))?,
                    temperature.ok_or_else(|| de::Error::missing_field("temperature"))?,
                ))
            }
        }

        deserializer.deserialize_any(DimensionVisitor)
    }
}

/// Compile-time marker implemented by quantity dimensions.
pub trait DimensionMarker: Clone + Copy + std::fmt::Debug + Eq + Send + Sync + 'static {
    /// Runtime counterpart of the marker.
    const DIMENSION: Dimension;
}

macro_rules! dimension_marker {
    ($name:ident, $dimension:ident, $doc:literal) => {
        #[doc = $doc]
        #[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
        pub struct $name;

        impl DimensionMarker for $name {
            const DIMENSION: Dimension = Dimension::$dimension;
        }
    };
}

dimension_marker!(
    Dimensionless,
    Dimensionless,
    "Marker for dimensionless quantities."
);
dimension_marker!(Time, Time, "Marker for time quantities.");
dimension_marker!(Length, Length, "Marker for length quantities.");
dimension_marker!(Pressure, Pressure, "Marker for pressure quantities.");
dimension_marker!(Mass, Mass, "Marker for mass quantities.");
dimension_marker!(
    Temperature,
    Temperature,
    "Marker for temperature quantities."
);
dimension_marker!(Velocity, Velocity, "Marker for velocity quantities.");

/// Named unit with an affine conversion into SI.
#[derive(Clone, Debug, PartialEq)]
pub struct Unit {
    symbol: String,
    dimension: Dimension,
    scale_to_si: f64,
    offset_to_si: f64,
}

impl Unit {
    /// Creates a unit definition.
    pub fn new(
        symbol: impl Into<String>,
        dimension: Dimension,
        scale_to_si: f64,
        offset_to_si: f64,
    ) -> Result<Self, QuantityError> {
        let symbol = symbol.into();
        if symbol.trim().is_empty() || !scale_to_si.is_finite() || scale_to_si == 0.0 {
            return Err(QuantityError::InvalidUnitDefinition);
        }
        if !offset_to_si.is_finite() {
            return Err(QuantityError::InvalidUnitDefinition);
        }
        Ok(Self {
            symbol,
            dimension,
            scale_to_si,
            offset_to_si,
        })
    }

    /// Returns the stable unit symbol.
    #[must_use]
    pub fn symbol(&self) -> &str {
        &self.symbol
    }

    /// Returns the physical dimension.
    #[must_use]
    pub const fn dimension(&self) -> Dimension {
        self.dimension
    }

    /// Returns the multiplicative scale into SI.
    #[must_use]
    pub const fn scale_to_si(&self) -> f64 {
        self.scale_to_si
    }

    /// Returns the additive offset into SI.
    #[must_use]
    pub const fn offset_to_si(&self) -> f64 {
        self.offset_to_si
    }

    /// Converts a finite value to SI.
    pub fn to_si(&self, value: f64) -> Result<f64, QuantityError> {
        if !value.is_finite() {
            return Err(QuantityError::NonFiniteValue);
        }
        Ok(value.mul_add(self.scale_to_si, self.offset_to_si))
    }

    /// Converts a finite SI value back into this unit.
    pub fn from_si(&self, value_si: f64) -> Result<f64, QuantityError> {
        if !value_si.is_finite() {
            return Err(QuantityError::NonFiniteValue);
        }
        Ok((value_si - self.offset_to_si) / self.scale_to_si)
    }
}

impl Serialize for Unit {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("Unit", 4)?;
        state.serialize_field("symbol", &self.symbol)?;
        state.serialize_field("dimension", &self.dimension)?;
        state.serialize_field("scale_to_si", &self.scale_to_si)?;
        state.serialize_field("offset_to_si", &self.offset_to_si)?;
        state.end()
    }
}

impl<'de> Deserialize<'de> for Unit {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct UnitWire {
            symbol: String,
            dimension: Dimension,
            scale_to_si: f64,
            offset_to_si: f64,
        }

        let wire = UnitWire::deserialize(deserializer)?;
        Unit::new(
            wire.symbol,
            wire.dimension,
            wire.scale_to_si,
            wire.offset_to_si,
        )
        .map_err(de::Error::custom)
    }
}

/// Typed physical quantity stored in SI while retaining the source unit.
#[derive(Clone, Debug, PartialEq)]
pub struct Quantity<D: DimensionMarker> {
    value_si: f64,
    source_unit: Unit,
    marker: PhantomData<D>,
}

impl<D: DimensionMarker> Quantity<D> {
    /// Validates a source value and converts it to SI.
    pub fn new(value: f64, source_unit: Unit) -> Result<Self, QuantityError> {
        if source_unit.dimension != D::DIMENSION {
            return Err(QuantityError::DimensionMismatch {
                expected: D::DIMENSION,
                actual: source_unit.dimension,
            });
        }
        let value_si = source_unit.to_si(value)?;
        Ok(Self {
            value_si,
            source_unit,
            marker: PhantomData,
        })
    }

    /// Builds a quantity from an already-normalized SI value and its source unit.
    pub fn from_si(value_si: f64, source_unit: Unit) -> Result<Self, QuantityError> {
        if source_unit.dimension != D::DIMENSION {
            return Err(QuantityError::DimensionMismatch {
                expected: D::DIMENSION,
                actual: source_unit.dimension,
            });
        }
        if !value_si.is_finite() {
            return Err(QuantityError::NonFiniteValue);
        }
        Ok(Self {
            value_si,
            source_unit,
            marker: PhantomData,
        })
    }

    /// Returns the normalized SI value.
    #[must_use]
    pub const fn value_si(&self) -> f64 {
        self.value_si
    }

    /// Returns the unit used by the input representation.
    #[must_use]
    pub const fn source_unit(&self) -> &Unit {
        &self.source_unit
    }

    /// Returns the value expressed in the source unit.
    pub fn value_in_source_unit(&self) -> Result<f64, QuantityError> {
        self.source_unit.from_si(self.value_si)
    }

    /// Resolves a [`QuantityInput`] using the provided registry.
    pub fn resolve(input: &QuantityInput, registry: &UnitRegistry) -> Result<Self, QuantityError> {
        let (value, unit_symbol) = input.split_value_unit()?;
        let unit = registry
            .get(unit_symbol.as_str())
            .ok_or_else(|| QuantityError::UnknownUnit(unit_symbol.clone()))?
            .clone();
        Self::new(value, unit)
    }

    /// Returns true when the SI value is finite and strictly positive.
    #[must_use]
    pub fn is_positive_finite(&self) -> bool {
        self.value_si.is_finite() && self.value_si > 0.0
    }
}

impl<D: DimensionMarker> Serialize for Quantity<D> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let value = self
            .value_in_source_unit()
            .map_err(serde::ser::Error::custom)?;
        let mut state = serializer.serialize_struct("Quantity", 2)?;
        state.serialize_field("value", &value)?;
        state.serialize_field("unit", self.source_unit.symbol())?;
        state.end()
    }
}

impl<'de, D: DimensionMarker> Deserialize<'de> for Quantity<D> {
    fn deserialize<Ds>(deserializer: Ds) -> Result<Self, Ds::Error>
    where
        Ds: Deserializer<'de>,
    {
        let input = QuantityInput::deserialize(deserializer)?;
        Self::resolve(&input, &UnitRegistry::standard()).map_err(de::Error::custom)
    }
}

/// Accepted unresolved quantity syntax.
#[derive(Clone, Debug, PartialEq)]
pub enum QuantityInput {
    /// Explicit value and unit object.
    Object {
        /// Numeric source value.
        value: f64,
        /// Unit symbol.
        unit: String,
    },
    /// Text shorthand such as `"900 s"`.
    Text(String),
}

impl QuantityInput {
    /// Creates an object-form input.
    #[must_use]
    pub fn object(value: f64, unit: impl Into<String>) -> Self {
        Self::Object {
            value,
            unit: unit.into(),
        }
    }

    /// Creates a text-form input.
    #[must_use]
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text(text.into())
    }

    /// Splits this input into a numeric value and unit symbol.
    pub fn split_value_unit(&self) -> Result<(f64, String), QuantityError> {
        match self {
            Self::Object { value, unit } => {
                if !value.is_finite() {
                    return Err(QuantityError::NonFiniteValue);
                }
                let unit = unit.trim();
                if unit.is_empty() {
                    return Err(QuantityError::InvalidText(format!(
                        "empty unit for value {value}"
                    )));
                }
                Ok((*value, unit.to_owned()))
            }
            Self::Text(text) => parse_quantity_text(text),
        }
    }
}

impl Serialize for QuantityInput {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Self::Object { value, unit } => {
                let mut state = serializer.serialize_struct("QuantityInput", 2)?;
                state.serialize_field("value", value)?;
                state.serialize_field("unit", unit)?;
                state.end()
            }
            Self::Text(text) => serializer.serialize_str(text),
        }
    }
}

impl<'de> Deserialize<'de> for QuantityInput {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct QuantityInputVisitor;

        impl<'de> Visitor<'de> for QuantityInputVisitor {
            type Value = QuantityInput;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("a quantity object {value, unit} or text like \"10 min\"")
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(QuantityInput::Text(value.to_owned()))
            }

            fn visit_string<E>(self, value: String) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(QuantityInput::Text(value))
            }

            fn visit_map<M>(self, mut map: M) -> Result<Self::Value, M::Error>
            where
                M: MapAccess<'de>,
            {
                let mut value = None;
                let mut unit = None;
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "value" => {
                            if value.is_some() {
                                return Err(de::Error::duplicate_field("value"));
                            }
                            value = Some(map.next_value::<f64>()?);
                        }
                        "unit" => {
                            if unit.is_some() {
                                return Err(de::Error::duplicate_field("unit"));
                            }
                            unit = Some(map.next_value::<String>()?);
                        }
                        other => {
                            return Err(de::Error::unknown_field(other, &["value", "unit"]));
                        }
                    }
                }
                let value = value.ok_or_else(|| de::Error::missing_field("value"))?;
                let unit = unit.ok_or_else(|| de::Error::missing_field("unit"))?;
                Ok(QuantityInput::Object { value, unit })
            }
        }

        deserializer.deserialize_any(QuantityInputVisitor)
    }
}

/// Registry shared by Case parsing and meteorology profile graphs.
#[derive(Clone, Debug, Default)]
pub struct UnitRegistry {
    units: BTreeMap<String, Unit>,
}

impl UnitRegistry {
    /// Creates an empty registry.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            units: BTreeMap::new(),
        }
    }

    /// Returns a registry preloaded with the v0 standard units.
    #[must_use]
    pub fn standard() -> Self {
        let mut registry = Self::new();
        let definitions = [
            ("1", Dimension::Dimensionless, 1.0, 0.0),
            ("s", Dimension::Time, 1.0, 0.0),
            ("min", Dimension::Time, 60.0, 0.0),
            ("h", Dimension::Time, 3600.0, 0.0),
            ("day", Dimension::Time, 86400.0, 0.0),
            ("m", Dimension::Length, 1.0, 0.0),
            ("km", Dimension::Length, 1000.0, 0.0),
            ("cm", Dimension::Length, 0.01, 0.0),
            ("um", Dimension::Length, 1.0e-6, 0.0),
            ("Pa", Dimension::Pressure, 1.0, 0.0),
            ("hPa", Dimension::Pressure, 100.0, 0.0),
            ("mbar", Dimension::Pressure, 100.0, 0.0),
            ("kg", Dimension::Mass, 1.0, 0.0),
            ("g", Dimension::Mass, 0.001, 0.0),
            ("K", Dimension::Temperature, 1.0, 0.0),
            ("degC", Dimension::Temperature, 1.0, 273.15),
            ("m/s", Dimension::Velocity, 1.0, 0.0),
            ("m_s-1", Dimension::Velocity, 1.0, 0.0),
        ];
        for (symbol, dimension, scale, offset) in definitions {
            if let Ok(unit) = Unit::new(symbol, dimension, scale, offset) {
                let _ = registry.register(unit);
            }
        }
        registry
    }

    /// Registers one unique symbol.
    pub fn register(&mut self, unit: Unit) -> Result<(), QuantityError> {
        if self.units.contains_key(unit.symbol()) {
            return Err(QuantityError::DuplicateUnit(unit.symbol().to_owned()));
        }
        self.units.insert(unit.symbol().to_owned(), unit);
        Ok(())
    }

    /// Looks up an exact unit symbol.
    #[must_use]
    pub fn get(&self, symbol: &str) -> Option<&Unit> {
        self.units.get(symbol)
    }

    /// Returns all registered symbols in deterministic order.
    #[must_use]
    pub fn symbols(&self) -> Vec<&str> {
        self.units.keys().map(String::as_str).collect()
    }

    /// Parses text or object input into a typed quantity.
    pub fn resolve<D: DimensionMarker>(
        &self,
        input: &QuantityInput,
    ) -> Result<Quantity<D>, QuantityError> {
        Quantity::<D>::resolve(input, self)
    }

    /// Parses a text shorthand into a typed quantity.
    pub fn parse_text<D: DimensionMarker>(&self, text: &str) -> Result<Quantity<D>, QuantityError> {
        self.resolve(&QuantityInput::Text(text.to_owned()))
    }
}

/// Quantity construction or unit-registry failure.
#[derive(Clone, Debug, PartialEq)]
pub enum QuantityError {
    /// A unit has an empty symbol or invalid affine conversion.
    InvalidUnitDefinition,
    /// The numeric input is NaN or infinite.
    NonFiniteValue,
    /// The unit dimension does not match the typed quantity.
    DimensionMismatch {
        /// Dimension required by the quantity type.
        expected: Dimension,
        /// Dimension declared by the source unit.
        actual: Dimension,
    },
    /// A registry already contains the exact symbol.
    DuplicateUnit(String),
    /// The unit symbol is not present in the registry.
    UnknownUnit(String),
    /// Text shorthand could not be parsed.
    InvalidText(String),
}

impl fmt::Display for QuantityError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidUnitDefinition => write!(f, "invalid unit definition"),
            Self::NonFiniteValue => write!(f, "quantity value must be finite"),
            Self::DimensionMismatch { expected, actual } => {
                write!(
                    f,
                    "dimension mismatch: expected {expected:?}, actual {actual:?}"
                )
            }
            Self::DuplicateUnit(symbol) => write!(f, "duplicate unit symbol: {symbol}"),
            Self::UnknownUnit(symbol) => write!(f, "unknown unit symbol: {symbol}"),
            Self::InvalidText(text) => write!(f, "invalid quantity text: {text}"),
        }
    }
}

impl std::error::Error for QuantityError {}

/// Parses `"<number> <unit>"` with a required whitespace separator.
fn parse_quantity_text(text: &str) -> Result<(f64, String), QuantityError> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err(QuantityError::InvalidText(text.to_owned()));
    }
    let Some((raw_value, raw_unit)) = trimmed.rsplit_once(char::is_whitespace) else {
        return Err(QuantityError::InvalidText(trimmed.to_owned()));
    };
    let value = f64::from_str(raw_value.trim())
        .map_err(|_| QuantityError::InvalidText(trimmed.to_owned()))?;
    if !value.is_finite() {
        return Err(QuantityError::NonFiniteValue);
    }
    let unit = raw_unit.trim();
    if unit.is_empty() {
        return Err(QuantityError::InvalidText(trimmed.to_owned()));
    }
    Ok((value, unit.to_owned()))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;

    #[test]
    fn unit_rejects_invalid_definitions() {
        assert!(Unit::new("", Dimension::Time, 1.0, 0.0).is_err());
        assert!(Unit::new("s", Dimension::Time, 0.0, 0.0).is_err());
        assert!(Unit::new("s", Dimension::Time, f64::NAN, 0.0).is_err());
        assert!(Unit::new("s", Dimension::Time, 1.0, f64::INFINITY).is_err());
    }

    #[test]
    fn unit_deserialize_rejects_zero_scale_yaml_and_json() {
        let yaml = r#"
symbol: bad
dimension: time
scale_to_si: 0
offset_to_si: 0
"#;
        assert!(serde_yml::from_str::<Unit>(yaml).is_err());
        let json = r#"{"symbol":"bad","dimension":"time","scale_to_si":0.0,"offset_to_si":0.0}"#;
        assert!(serde_json::from_str::<Unit>(json).is_err());
    }

    #[test]
    fn unit_deserialize_rejects_empty_symbol_and_unknown_field() {
        let json = r#"{"symbol":"","dimension":"time","scale_to_si":1.0,"offset_to_si":0.0}"#;
        assert!(serde_json::from_str::<Unit>(json).is_err());
        let json =
            r#"{"symbol":"s","dimension":"time","scale_to_si":1.0,"offset_to_si":0.0,"extra":1}"#;
        assert!(serde_json::from_str::<Unit>(json).is_err());
    }

    #[test]
    fn dimension_serde_preserves_named_strings_and_stabilizes_compounds() {
        assert_eq!(
            serde_json::to_string(&Dimension::TIME).unwrap(),
            r#""time""#
        );
        assert_eq!(
            serde_json::from_str::<Dimension>(r#""velocity""#).unwrap(),
            Dimension::VELOCITY
        );
        let density = serde_json::to_string(&Dimension::DENSITY).unwrap();
        assert_eq!(
            density,
            r#"{"mass":1,"length":-3,"time":0,"temperature":0}"#
        );
        assert_eq!(
            serde_json::from_str::<Dimension>(&density).unwrap(),
            Dimension::DENSITY
        );
        assert!(serde_json::from_str::<Dimension>(r#"{"mass":1}"#).is_err());
    }

    #[test]
    fn compound_dimension_arithmetic_is_shared_with_units() {
        assert_eq!(
            Dimension::PRESSURE.checked_quotient(Dimension::TIME),
            Some(Dimension::PRESSURE_TENDENCY)
        );
        assert_eq!(
            Dimension::MASS.checked_quotient(Dimension::LENGTH.checked_power(3).unwrap()),
            Some(Dimension::DENSITY)
        );
        let unit = Unit::new("kg/m3", Dimension::DENSITY, 1.0, 0.0).unwrap();
        assert_eq!(unit.dimension(), Dimension::DENSITY);
    }

    #[test]
    fn time_units_convert_to_si() {
        let registry = UnitRegistry::standard();
        let ten_min = registry.parse_text::<Time>("10 min").expect("parse 10 min");
        assert!((ten_min.value_si() - 600.0).abs() < 1e-12);
        let two_hours = registry.resolve::<Time>(&QuantityInput::object(2.0, "h"));
        assert!((two_hours.expect("2 h").value_si() - 7200.0).abs() < 1e-12);
    }

    #[test]
    fn celsius_uses_affine_offset() {
        let registry = UnitRegistry::standard();
        let q = registry
            .resolve::<Temperature>(&QuantityInput::object(0.0, "degC"))
            .expect("0 degC");
        assert!((q.value_si() - 273.15).abs() < 1e-12);
    }

    #[test]
    fn dimension_mismatch_is_reported() {
        let registry = UnitRegistry::standard();
        let err = registry
            .resolve::<Time>(&QuantityInput::object(1.0, "m"))
            .expect_err("length is not time");
        assert_eq!(
            err,
            QuantityError::DimensionMismatch {
                expected: Dimension::Time,
                actual: Dimension::Length,
            }
        );
    }

    #[test]
    fn unknown_unit_and_bad_text() {
        let registry = UnitRegistry::standard();
        assert!(matches!(
            registry.parse_text::<Time>("10 fortnights"),
            Err(QuantityError::UnknownUnit(_))
        ));
        assert!(matches!(
            parse_quantity_text("10min"),
            Err(QuantityError::InvalidText(_))
        ));
        assert!(matches!(
            parse_quantity_text(""),
            Err(QuantityError::InvalidText(_))
        ));
    }

    #[test]
    fn duplicate_registration_fails() {
        let mut registry = UnitRegistry::new();
        let unit = Unit::new("s", Dimension::Time, 1.0, 0.0).unwrap();
        registry.register(unit.clone()).unwrap();
        assert_eq!(
            registry.register(unit),
            Err(QuantityError::DuplicateUnit("s".into()))
        );
    }

    #[test]
    fn quantity_input_serde_object_and_text() {
        let object: QuantityInput = serde_json::from_str(r#"{"value":10.0,"unit":"min"}"#).unwrap();
        assert_eq!(object, QuantityInput::object(10.0, "min"));
        let text: QuantityInput = serde_json::from_str(r#""900 s""#).unwrap();
        assert_eq!(text, QuantityInput::text("900 s"));
        assert!(
            serde_json::from_str::<QuantityInput>(r#"{"value":1.0,"unit":"s","extra":true}"#)
                .is_err()
        );
    }

    #[test]
    fn typed_quantity_serde_roundtrip_uses_standard_registry() {
        let original = UnitRegistry::standard()
            .parse_text::<Time>("15 min")
            .unwrap();
        let json = serde_json::to_string(&original).unwrap();
        let restored: Quantity<Time> = serde_json::from_str(&json).unwrap();
        assert!((restored.value_si() - 900.0).abs() < 1e-12);
        assert_eq!(restored.source_unit().symbol(), "min");
    }

    #[test]
    fn non_finite_values_are_rejected() {
        let unit = Unit::new("s", Dimension::Time, 1.0, 0.0).unwrap();
        assert_eq!(
            Quantity::<Time>::new(f64::NAN, unit),
            Err(QuantityError::NonFiniteValue)
        );
    }

    #[test]
    fn standard_registry_symbols_are_deterministic() {
        let registry = UnitRegistry::standard();
        let symbols = registry.symbols();
        let mut sorted = symbols.clone();
        sorted.sort_unstable();
        assert_eq!(symbols, sorted);
        assert!(symbols.contains(&"min"));
        assert!(symbols.contains(&"degC"));
    }
}
