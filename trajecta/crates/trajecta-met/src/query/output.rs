//! # Contract: structure-of-arrays query output
//!
//! Floating-point columns contain finite scientific values only. Per-point
//! status and per-field validity are independent typed channels; NaN,
//! infinity, sentinel numbers, and magic large values are never used to carry
//! status. Invalid numeric slots remain finite but have no scientific meaning.

use std::collections::BTreeSet;
use std::sync::Arc;

use trajecta_case::model::meteorology::DomainId;
use trajecta_case::model::physics::ModelId;

use crate::field::{FieldKey, FieldQuality};
use crate::grid::{CellId, GridPoint};
use crate::io::inventory::LogicalFrameId;
use crate::provenance::{ProvenanceId, ProvenanceRecord, ProvenanceTable};
use crate::vertical::VerticalBounds;

/// Recoverable status for one query point.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum SampleStatus {
    /// Every requested field is valid.
    Ok,
    /// No domain safely covers the point.
    OutOfDomain,
    /// Longitude has no unique local east direction at an exact pole.
    PolarSingularity,
    /// Requested AGL/ASL height or pressure lies below the local surface.
    BelowGround,
    /// The requested near-surface height is outside the surface model domain.
    SurfaceLayerUndefined,
    /// Requested coordinate lies above the highest level carried by the data.
    AboveAvailableTop,
    /// Requested coordinate lies above the declared physical model top.
    AboveModelTop,
    /// Native vertical support is structurally invalid.
    InvalidVerticalColumn,
    /// Deterministic arithmetic failed a finite or monotonicity check.
    NumericalFailure,
}

/// Compact per-point validity bits for one value column.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ValidityMask {
    len: usize,
    words: Vec<u64>,
}

impl ValidityMask {
    /// Creates a mask whose entries are all invalid.
    #[must_use]
    pub fn all_invalid(len: usize) -> Self {
        Self {
            len,
            words: vec![0; word_count(len)],
        }
    }

    /// Creates a mask whose entries are all valid.
    #[must_use]
    pub fn all_valid(len: usize) -> Self {
        let mut words = vec![u64::MAX; word_count(len)];
        clear_unused_bits(&mut words, len);
        Self { len, words }
    }

    /// Packs booleans into the stable least-significant-bit-first layout.
    #[must_use]
    pub fn from_bools(values: &[bool]) -> Self {
        let mut mask = Self::all_invalid(values.len());
        for (index, value) in values.iter().copied().enumerate() {
            if value {
                let word = index / u64::BITS as usize;
                let bit = index % u64::BITS as usize;
                mask.words[word] |= 1_u64 << bit;
            }
        }
        mask
    }

    /// Returns the number of represented samples.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.len
    }

    /// Returns whether the mask contains no samples.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Returns one validity bit, or `None` when the index is out of range.
    #[must_use]
    pub fn get(&self, index: usize) -> Option<bool> {
        if index >= self.len {
            return None;
        }
        let word = index / u64::BITS as usize;
        let bit = index % u64::BITS as usize;
        Some(self.words[word] & (1_u64 << bit) != 0)
    }

    /// Updates one validity bit while preserving canonical unused bits.
    pub fn set(&mut self, index: usize, valid: bool) -> Result<(), OutputError> {
        if index >= self.len {
            return Err(OutputError::IndexOutOfBounds {
                index,
                len: self.len,
            });
        }
        let word = index / u64::BITS as usize;
        let bit = index % u64::BITS as usize;
        if valid {
            self.words[word] |= 1_u64 << bit;
        } else {
            self.words[word] &= !(1_u64 << bit);
        }
        Ok(())
    }

    /// Iterates validity in caller order.
    pub fn iter(&self) -> impl Iterator<Item = bool> + '_ {
        (0..self.len).map(|index| {
            let word = index / u64::BITS as usize;
            let bit = index % u64::BITS as usize;
            self.words[word] & (1_u64 << bit) != 0
        })
    }

    /// Returns the number of valid samples.
    #[must_use]
    pub fn count_valid(&self) -> usize {
        self.words
            .iter()
            .map(|word| word.count_ones() as usize)
            .sum()
    }

    /// Returns the canonical packed representation for hashing or backends.
    #[must_use]
    pub fn words(&self) -> &[u64] {
        &self.words
    }
}

fn word_count(len: usize) -> usize {
    len.saturating_add(u64::BITS as usize - 1) / u64::BITS as usize
}

fn clear_unused_bits(words: &mut [u64], len: usize) {
    let used = len % u64::BITS as usize;
    if used != 0
        && let Some(last) = words.last_mut()
    {
        *last &= (1_u64 << used) - 1;
    }
}

/// Finite values and their independent metadata in caller order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ValueColumn {
    values: Vec<f64>,
    validity: ValidityMask,
    quality: Vec<FieldQuality>,
    provenance: Vec<ProvenanceId>,
}

impl ValueColumn {
    /// Validates and constructs one output value column.
    pub fn new(
        values: Vec<f64>,
        validity: ValidityMask,
        quality: Vec<FieldQuality>,
        provenance: Vec<ProvenanceId>,
    ) -> Result<Self, OutputError> {
        let len = values.len();
        if validity.len() != len || quality.len() != len || provenance.len() != len {
            return Err(OutputError::ValueColumnLengthMismatch {
                values: len,
                validity: validity.len(),
                quality: quality.len(),
                provenance: provenance.len(),
            });
        }
        if values.iter().any(|value| !value.is_finite()) {
            return Err(OutputError::NonFiniteValue);
        }
        Ok(Self {
            values,
            validity,
            quality,
            provenance,
        })
    }

    /// Constructs a column whose samples share quality and provenance.
    pub fn uniform_metadata(
        values: Vec<f64>,
        validity: ValidityMask,
        quality: FieldQuality,
        provenance: ProvenanceId,
    ) -> Result<Self, OutputError> {
        let len = values.len();
        Self::new(values, validity, vec![quality; len], vec![provenance; len])
    }

    /// Returns the sample count.
    #[must_use]
    pub fn len(&self) -> usize {
        self.values.len()
    }

    /// Returns whether the column is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    /// Returns all finite numeric slots in caller order.
    #[must_use]
    pub fn values(&self) -> &[f64] {
        &self.values
    }

    /// Returns the independent validity bits.
    #[must_use]
    pub const fn validity(&self) -> &ValidityMask {
        &self.validity
    }

    /// Returns source, derived, or estimated quality per sample.
    #[must_use]
    pub fn quality(&self) -> &[FieldQuality] {
        &self.quality
    }

    /// Returns compact provenance identifiers per sample.
    #[must_use]
    pub fn provenance(&self) -> &[ProvenanceId] {
        &self.provenance
    }

    /// Returns one scientific value only when the corresponding bit is valid.
    #[must_use]
    pub fn value(&self, index: usize) -> Option<f64> {
        self.validity
            .get(index)
            .filter(|valid| *valid)
            .and_then(|_| self.values.get(index).copied())
    }
}

/// One requested field and its value column.
#[derive(Clone, Debug, PartialEq)]
pub struct FieldColumn {
    field: FieldKey,
    samples: ValueColumn,
}

impl FieldColumn {
    /// Creates one field column from already-validated samples.
    #[must_use]
    pub const fn new(field: FieldKey, samples: ValueColumn) -> Self {
        Self { field, samples }
    }

    /// Returns the exact field identity.
    #[must_use]
    pub const fn field(&self) -> &FieldKey {
        &self.field
    }

    /// Returns the field's value and metadata column.
    #[must_use]
    pub const fn samples(&self) -> &ValueColumn {
        &self.samples
    }
}

/// One typed status per input point.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct StatusColumn {
    values: Vec<SampleStatus>,
}

impl StatusColumn {
    /// Creates a status column in caller order.
    #[must_use]
    pub const fn new(values: Vec<SampleStatus>) -> Self {
        Self { values }
    }

    /// Returns statuses in caller order.
    #[must_use]
    pub fn values(&self) -> &[SampleStatus] {
        &self.values
    }

    /// Returns the number of statuses.
    #[must_use]
    pub fn len(&self) -> usize {
        self.values.len()
    }

    /// Returns whether the column is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    /// Returns one status.
    #[must_use]
    pub fn get(&self, index: usize) -> Option<SampleStatus> {
        self.values.get(index).copied()
    }
}

/// Optional local vertical bounds per input point.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BoundsColumn {
    values: Vec<Option<VerticalBounds>>,
}

impl BoundsColumn {
    /// Creates a bounds column; `None` is reserved for points with no domain.
    #[must_use]
    pub const fn new(values: Vec<Option<VerticalBounds>>) -> Self {
        Self { values }
    }

    /// Returns local bounds in caller order.
    #[must_use]
    pub fn values(&self) -> &[Option<VerticalBounds>] {
        &self.values
    }

    /// Returns one local bounds record.
    #[must_use]
    pub fn get(&self, index: usize) -> Option<&VerticalBounds> {
        self.values.get(index).and_then(Option::as_ref)
    }

    /// Returns the number of entries.
    #[must_use]
    pub fn len(&self) -> usize {
        self.values.len()
    }

    /// Returns whether the column is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }
}

/// Horizontal interpolation method used by one explained level.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ExplainHorizontalMethod {
    /// Ordinary four-corner bilinear support.
    Bilinear,
    /// Three valid corners forming a containing triangle.
    ValidTriangle,
    /// Endpoint supports differ and the displayed weights are time-blended.
    TimeBlended,
}

/// Exact horizontal support used at the explained vertical bracket.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ExplainHorizontalSupport {
    /// Four source points in deterministic storage order.
    pub points: [GridPoint; 4],
    /// Weights used at the first vertical level.
    pub first_level_weights: [f64; 4],
    /// Method used at the first vertical level.
    pub first_level_method: ExplainHorizontalMethod,
    /// Weights used at the second level when it differs from the first.
    pub second_level_weights: Option<[f64; 4]>,
    /// Method used at the second level when it differs from the first.
    pub second_level_method: Option<ExplainHorizontalMethod>,
}

/// Vertical route selected for a point.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ExplainVerticalPath {
    /// Linear interpolation in geometric height.
    LinearHeight,
    /// Linear interpolation in logarithmic pressure.
    LogPressure,
    /// Dedicated near-surface similarity model.
    SurfaceLayer,
    /// No valid bracket was available for this point.
    Unavailable,
}

/// Exact vertical bracket and interpolation route.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ExplainVerticalSupport {
    /// Selected route.
    pub path: ExplainVerticalPath,
    /// First native full-level index in top-to-surface order.
    pub first_level: Option<usize>,
    /// Second native full-level index in top-to-surface order.
    pub second_level: Option<usize>,
    /// Weight assigned to the second level.
    pub second_weight: Option<f64>,
}

/// Field-level quality and compact provenance identity.
#[derive(Clone, Debug, PartialEq)]
pub struct ExplainField {
    /// Exact output field.
    pub field: FieldKey,
    /// Source, derived, or estimated quality used at this point.
    pub quality: FieldQuality,
    /// Compact identifier resolved through the output provenance table.
    pub provenance: ProvenanceId,
}

/// Optional explanation of domain, frames, interpolation, model, and provenance.
#[derive(Clone, Debug, PartialEq)]
pub struct ExplainRecord {
    /// Selected domain.
    pub domain: DomainId,
    /// Stable horizontal cell.
    pub cell: CellId,
    /// Earlier physical frame.
    pub before_frame: LogicalFrameId,
    /// Later physical frame.
    pub after_frame: LogicalFrameId,
    /// Earlier frame weight.
    pub before_weight: f64,
    /// Later frame weight.
    pub after_weight: f64,
    /// Horizontal points and weights used by the selected route.
    pub horizontal: ExplainHorizontalSupport,
    /// Vertical route and bracket.
    pub vertical: ExplainVerticalSupport,
    /// Near-surface model when the surface route was selected.
    pub surface_model: Option<ModelId>,
    /// Field-level quality and provenance in plan order.
    pub fields: Vec<ExplainField>,
}

/// Complete generic SoA query result in original caller order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct QueryOutput {
    fields: Vec<FieldColumn>,
    status: StatusColumn,
    bounds: BoundsColumn,
    provenance: Arc<ProvenanceTable>,
    explain: Option<Vec<Option<ExplainRecord>>>,
}

impl QueryOutput {
    /// Validates consistent lengths and unique field identities.
    pub fn new(
        fields: Vec<FieldColumn>,
        status: StatusColumn,
        bounds: BoundsColumn,
        provenance: Arc<ProvenanceTable>,
        explain: Option<Vec<Option<ExplainRecord>>>,
    ) -> Result<Self, OutputError> {
        let len = status.len();
        let mut identities = BTreeSet::new();
        for field in &fields {
            if !identities.insert(field.field().clone()) {
                return Err(OutputError::DuplicateField(field.field().clone()));
            }
            ensure_length(
                format!("field:{:?}", field.field()),
                len,
                field.samples().len(),
            )?;
            validate_column_provenance(field.field(), field.samples(), &provenance)?;
        }
        ensure_length("bounds".into(), len, bounds.len())?;
        if let Some(records) = &explain {
            ensure_length("explain".into(), len, records.len())?;
            validate_explain_provenance(records, &provenance)?;
        }
        Ok(Self {
            fields,
            status,
            bounds,
            provenance,
            explain,
        })
    }

    /// Returns requested field columns in stable plan order.
    #[must_use]
    pub fn fields(&self) -> &[FieldColumn] {
        &self.fields
    }

    /// Returns the typed point-status column.
    #[must_use]
    pub const fn status(&self) -> &StatusColumn {
        &self.status
    }

    /// Returns local vertical bounds per point.
    #[must_use]
    pub const fn bounds(&self) -> &BoundsColumn {
        &self.bounds
    }

    /// Returns the output-level table resolved by every compact provenance ID.
    #[must_use]
    pub const fn provenance(&self) -> &Arc<ProvenanceTable> {
        &self.provenance
    }

    /// Returns optional point explanations.
    #[must_use]
    pub fn explain(&self) -> Option<&[Option<ExplainRecord>]> {
        self.explain.as_deref()
    }

    /// Returns a read-only row view when the index is in range.
    #[must_use]
    pub fn row(&self, index: usize) -> Option<SampleView<'_>> {
        (index < self.status.len()).then_some(SampleView {
            output: self,
            index,
        })
    }
}

/// Read-only convenience view over one generic output row.
#[derive(Clone, Copy, Debug)]
pub struct SampleView<'a> {
    output: &'a QueryOutput,
    index: usize,
}

impl<'a> SampleView<'a> {
    /// Returns the point status.
    #[must_use]
    pub fn status(self) -> SampleStatus {
        self.output.status.values[self.index]
    }

    /// Returns a valid field value by exact key.
    #[must_use]
    pub fn value(self, field: &FieldKey) -> Option<f64> {
        self.output
            .fields
            .iter()
            .find(|column| column.field() == field)
            .and_then(|column| column.samples().value(self.index))
    }

    /// Returns field quality by exact key even when the value is invalid.
    #[must_use]
    pub fn quality(self, field: &FieldKey) -> Option<FieldQuality> {
        self.output
            .fields
            .iter()
            .find(|column| column.field() == field)
            .and_then(|column| column.samples().quality().get(self.index).copied())
    }

    /// Returns field provenance by exact key even when the value is invalid.
    #[must_use]
    pub fn provenance(self, field: &FieldKey) -> Option<ProvenanceId> {
        self.output
            .fields
            .iter()
            .find(|column| column.field() == field)
            .and_then(|column| column.samples().provenance().get(self.index).copied())
    }

    /// Resolves field provenance through this output's self-contained table.
    #[must_use]
    pub fn provenance_record(self, field: &FieldKey) -> Option<&'a ProvenanceRecord> {
        self.provenance(field)
            .and_then(|id| self.output.provenance.get(id))
    }

    /// Returns local vertical bounds when a domain was available.
    #[must_use]
    pub fn bounds(self) -> Option<VerticalBounds> {
        self.output.bounds.get(self.index).copied()
    }
}

/// Eight strongly typed value columns required by M4 transport.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TransportColumns {
    /// Eastward wind in metres per second.
    pub eastward_wind_m_s: ValueColumn,
    /// Northward wind in metres per second.
    pub northward_wind_m_s: ValueColumn,
    /// Geometric vertical velocity, positive upward, in metres per second.
    pub geometric_vertical_velocity_m_s: ValueColumn,
    /// Air pressure in pascals.
    pub air_pressure_pa: ValueColumn,
    /// Air temperature in kelvin.
    pub air_temperature_k: ValueColumn,
    /// Specific humidity as a mass fraction.
    pub specific_humidity: ValueColumn,
    /// Moist-air density in kilograms per cubic metre.
    pub air_density_kg_m3: ValueColumn,
    /// Geometric terrain height above mean sea level in metres.
    pub terrain_height_asl_m: ValueColumn,
}

impl TransportColumns {
    fn named_columns(&self) -> [(&'static str, &ValueColumn); 8] {
        [
            ("eastward_wind_m_s", &self.eastward_wind_m_s),
            ("northward_wind_m_s", &self.northward_wind_m_s),
            (
                "geometric_vertical_velocity_m_s",
                &self.geometric_vertical_velocity_m_s,
            ),
            ("air_pressure_pa", &self.air_pressure_pa),
            ("air_temperature_k", &self.air_temperature_k),
            ("specific_humidity", &self.specific_humidity),
            ("air_density_kg_m3", &self.air_density_kg_m3),
            ("terrain_height_asl_m", &self.terrain_height_asl_m),
        ]
    }
}

/// Strongly typed complete transport result consumed by M4.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TransportOutput {
    columns: TransportColumns,
    status: StatusColumn,
    bounds: BoundsColumn,
    provenance: Arc<ProvenanceTable>,
    explain: Option<Vec<Option<ExplainRecord>>>,
}

impl TransportOutput {
    /// Validates all eight transport columns against the point count.
    pub fn new(
        columns: TransportColumns,
        status: StatusColumn,
        bounds: BoundsColumn,
        provenance: Arc<ProvenanceTable>,
        explain: Option<Vec<Option<ExplainRecord>>>,
    ) -> Result<Self, OutputError> {
        let len = status.len();
        for (name, column) in columns.named_columns() {
            ensure_length(name.into(), len, column.len())?;
        }
        for (field, column) in transport_field_columns(&columns) {
            validate_column_provenance(&field, column, &provenance)?;
        }
        ensure_length("bounds".into(), len, bounds.len())?;
        if let Some(records) = &explain {
            ensure_length("explain".into(), len, records.len())?;
            validate_explain_provenance(records, &provenance)?;
        }
        Ok(Self {
            columns,
            status,
            bounds,
            provenance,
            explain,
        })
    }

    /// Returns the eight named value columns.
    #[must_use]
    pub const fn columns(&self) -> &TransportColumns {
        &self.columns
    }

    /// Returns typed point statuses.
    #[must_use]
    pub const fn status(&self) -> &StatusColumn {
        &self.status
    }

    /// Returns local vertical bounds.
    #[must_use]
    pub const fn bounds(&self) -> &BoundsColumn {
        &self.bounds
    }

    /// Returns the output-level table resolved by every compact provenance ID.
    #[must_use]
    pub const fn provenance(&self) -> &Arc<ProvenanceTable> {
        &self.provenance
    }

    /// Returns optional point explanations.
    #[must_use]
    pub fn explain(&self) -> Option<&[Option<ExplainRecord>]> {
        self.explain.as_deref()
    }

    /// Returns one strongly typed row view.
    #[must_use]
    pub fn row(&self, index: usize) -> Option<TransportSampleView<'_>> {
        (index < self.status.len()).then_some(TransportSampleView {
            output: self,
            index,
        })
    }
}

/// Read-only row view over a complete transport result.
#[derive(Clone, Copy, Debug)]
pub struct TransportSampleView<'a> {
    output: &'a TransportOutput,
    index: usize,
}

impl TransportSampleView<'_> {
    /// Returns point status.
    #[must_use]
    pub fn status(self) -> SampleStatus {
        self.output.status.values[self.index]
    }

    /// Returns valid eastward wind.
    #[must_use]
    pub fn eastward_wind_m_s(self) -> Option<f64> {
        self.output.columns.eastward_wind_m_s.value(self.index)
    }

    /// Returns valid northward wind.
    #[must_use]
    pub fn northward_wind_m_s(self) -> Option<f64> {
        self.output.columns.northward_wind_m_s.value(self.index)
    }

    /// Returns valid geometric vertical velocity.
    #[must_use]
    pub fn geometric_vertical_velocity_m_s(self) -> Option<f64> {
        self.output
            .columns
            .geometric_vertical_velocity_m_s
            .value(self.index)
    }

    /// Returns valid air pressure.
    #[must_use]
    pub fn air_pressure_pa(self) -> Option<f64> {
        self.output.columns.air_pressure_pa.value(self.index)
    }

    /// Returns valid air temperature.
    #[must_use]
    pub fn air_temperature_k(self) -> Option<f64> {
        self.output.columns.air_temperature_k.value(self.index)
    }

    /// Returns valid specific humidity.
    #[must_use]
    pub fn specific_humidity(self) -> Option<f64> {
        self.output.columns.specific_humidity.value(self.index)
    }

    /// Returns valid moist-air density.
    #[must_use]
    pub fn air_density_kg_m3(self) -> Option<f64> {
        self.output.columns.air_density_kg_m3.value(self.index)
    }

    /// Returns valid geometric terrain height.
    #[must_use]
    pub fn terrain_height_asl_m(self) -> Option<f64> {
        self.output.columns.terrain_height_asl_m.value(self.index)
    }

    /// Returns local vertical bounds when a domain was available.
    #[must_use]
    pub fn bounds(self) -> Option<VerticalBounds> {
        self.output.bounds.get(self.index).copied()
    }
}

fn ensure_length(component: String, expected: usize, actual: usize) -> Result<(), OutputError> {
    if actual != expected {
        return Err(OutputError::OutputLengthMismatch {
            component,
            expected,
            actual,
        });
    }
    Ok(())
}

fn validate_column_provenance(
    field: &FieldKey,
    column: &ValueColumn,
    table: &ProvenanceTable,
) -> Result<(), OutputError> {
    for (quality, id) in column.quality().iter().zip(column.provenance()) {
        let record = table.get(*id).ok_or(OutputError::MissingProvenance(*id))?;
        if &record.field != field || record.quality != *quality {
            return Err(OutputError::ProvenanceMismatch {
                field: field.clone(),
                id: *id,
            });
        }
    }
    Ok(())
}

fn validate_explain_provenance(
    records: &[Option<ExplainRecord>],
    table: &ProvenanceTable,
) -> Result<(), OutputError> {
    for field in records
        .iter()
        .flatten()
        .flat_map(|record| record.fields.iter())
    {
        let provenance = table
            .get(field.provenance)
            .ok_or(OutputError::MissingProvenance(field.provenance))?;
        if provenance.field != field.field || provenance.quality != field.quality {
            return Err(OutputError::ProvenanceMismatch {
                field: field.field.clone(),
                id: field.provenance,
            });
        }
    }
    Ok(())
}

fn transport_field_columns(columns: &TransportColumns) -> [(FieldKey, &ValueColumn); 8] {
    use crate::field::CanonicalField as Field;

    [
        (
            FieldKey::Canonical(Field::EastwardWind),
            &columns.eastward_wind_m_s,
        ),
        (
            FieldKey::Canonical(Field::NorthwardWind),
            &columns.northward_wind_m_s,
        ),
        (
            FieldKey::Canonical(Field::GeometricVerticalVelocity),
            &columns.geometric_vertical_velocity_m_s,
        ),
        (
            FieldKey::Canonical(Field::AirPressure),
            &columns.air_pressure_pa,
        ),
        (
            FieldKey::Canonical(Field::AirTemperature),
            &columns.air_temperature_k,
        ),
        (
            FieldKey::Canonical(Field::SpecificHumidity),
            &columns.specific_humidity,
        ),
        (
            FieldKey::Canonical(Field::AirDensity),
            &columns.air_density_kg_m3,
        ),
        (
            FieldKey::Canonical(Field::GeometricTerrainHeight),
            &columns.terrain_height_asl_m,
        ),
    ]
}

/// Output construction or indexing failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum OutputError {
    /// Values, validity, quality, and provenance have inconsistent lengths.
    ValueColumnLengthMismatch {
        /// Numeric value count.
        values: usize,
        /// Validity bit count.
        validity: usize,
        /// Quality count.
        quality: usize,
        /// Provenance count.
        provenance: usize,
    },
    /// A value slot contains NaN or infinity.
    NonFiniteValue,
    /// One query output component has the wrong point count.
    OutputLengthMismatch {
        /// Stable component name.
        component: String,
        /// Required point count.
        expected: usize,
        /// Actual component count.
        actual: usize,
    },
    /// The same field identity occurs more than once.
    DuplicateField(FieldKey),
    /// A compact provenance ID cannot be resolved by the output-level table.
    MissingProvenance(ProvenanceId),
    /// A provenance record names a different field or quality than its sample.
    ProvenanceMismatch {
        /// Output field whose sample metadata is inconsistent.
        field: FieldKey,
        /// Resolved compact identifier.
        id: ProvenanceId,
    },
    /// A validity index is outside the represented range.
    IndexOutOfBounds {
        /// Requested index.
        index: usize,
        /// Represented sample count.
        len: usize,
    },
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use crate::field::CanonicalField;
    use crate::provenance::ProvenanceRecord;

    fn provenance_for(field: FieldKey, quality: FieldQuality) -> Arc<ProvenanceTable> {
        let mut table = ProvenanceTable::new();
        table
            .intern(ProvenanceRecord {
                field,
                quality,
                sources: vec!["test-source".into()],
                transforms: Vec::new(),
                fallback_reason: None,
                profile_sha256: "test-profile".into(),
            })
            .unwrap();
        Arc::new(table)
    }

    #[test]
    fn validity_mask_packs_canonically_across_word_boundary() {
        let values = (0..70).map(|index| index % 3 == 0).collect::<Vec<_>>();
        let mask = ValidityMask::from_bools(&values);
        assert_eq!(mask.len(), 70);
        assert_eq!(mask.iter().collect::<Vec<_>>(), values);
        assert_eq!(mask.count_valid(), 24);
        assert_eq!(mask.words().len(), 2);
        assert_eq!(mask.words()[1] >> 6, 0);
    }

    #[test]
    fn invalid_slots_remain_finite_and_are_not_returned_as_values() {
        let column = ValueColumn::uniform_metadata(
            vec![12.0, 0.0],
            ValidityMask::from_bools(&[true, false]),
            FieldQuality::Source,
            ProvenanceId(7),
        )
        .unwrap();
        assert_eq!(column.value(0), Some(12.0));
        assert_eq!(column.value(1), None);
        assert!(
            ValueColumn::uniform_metadata(
                vec![f64::NAN],
                ValidityMask::all_invalid(1),
                FieldQuality::Source,
                ProvenanceId(0),
            )
            .is_err()
        );
    }

    #[test]
    fn query_output_rejects_duplicate_fields_and_length_drift() {
        let field = FieldKey::Canonical(CanonicalField::AirTemperature);
        let samples = ValueColumn::uniform_metadata(
            vec![280.0],
            ValidityMask::all_valid(1),
            FieldQuality::Source,
            ProvenanceId(0),
        )
        .unwrap();
        let duplicate = QueryOutput::new(
            vec![
                FieldColumn::new(field.clone(), samples.clone()),
                FieldColumn::new(field.clone(), samples),
            ],
            StatusColumn::new(vec![SampleStatus::Ok]),
            BoundsColumn::new(vec![None]),
            provenance_for(field.clone(), FieldQuality::Source),
            None,
        );
        assert_eq!(duplicate, Err(OutputError::DuplicateField(field)));
    }

    #[test]
    fn output_provenance_is_self_contained_and_field_checked() {
        let field = FieldKey::Canonical(CanonicalField::AirTemperature);
        let samples = ValueColumn::uniform_metadata(
            vec![280.0],
            ValidityMask::all_valid(1),
            FieldQuality::Source,
            ProvenanceId(0),
        )
        .unwrap();
        let output = QueryOutput::new(
            vec![FieldColumn::new(field.clone(), samples.clone())],
            StatusColumn::new(vec![SampleStatus::Ok]),
            BoundsColumn::new(vec![None]),
            provenance_for(field.clone(), FieldQuality::Source),
            None,
        )
        .unwrap();
        assert_eq!(
            output
                .row(0)
                .unwrap()
                .provenance_record(&field)
                .unwrap()
                .field,
            field
        );

        let wrong = FieldKey::Canonical(CanonicalField::SpecificHumidity);
        assert!(matches!(
            QueryOutput::new(
                vec![FieldColumn::new(field.clone(), samples)],
                StatusColumn::new(vec![SampleStatus::Ok]),
                BoundsColumn::new(vec![None]),
                provenance_for(wrong, FieldQuality::Source),
                None,
            ),
            Err(OutputError::ProvenanceMismatch { .. })
        ));
    }
}
