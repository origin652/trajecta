//! # Contract: immutable meteorological frames and prepared windows
//!
//! Decoded frames are validated and immutable after publication. Each field
//! owns finite f64 values, a separate validity mask, canonical `[y][x]` or
//! `[level][y][x]` layout, explicit temporal support, quality, unit, and compact
//! provenance identity. Published frames never trigger hidden source I/O.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, Weak};

use trajecta_case::model::meteorology::DomainId;
use trajecta_case::model::time::Timestamp;

use crate::field::{FieldKey, FieldQuality, FieldShape};
use crate::grid::DomainGeometry;
use crate::io::inventory::LogicalFrameId;
use crate::profile::graph::{
    ComputationGraph, ExecutionPlan, ExecutionStage, GraphNode, GraphNodeId, GraphOp, GraphUnit,
    ThermodynamicOp,
};
use crate::provenance::{ProvenanceId, ProvenanceTable};
use crate::query::cache::ColumnCache;
use crate::science::{M3_CONSTANTS, two_metre_specific_humidity_from_dewpoint_si};
use crate::vertical::VerticalTopology;

/// Canonical in-memory array layout.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ArrayLayout {
    /// One scalar value.
    Scalar,
    /// Two-dimensional row-major `[y][x]` array.
    Horizontal2D {
        /// Latitude or row count.
        ny: usize,
        /// Longitude or column count.
        nx: usize,
    },
    /// Three-dimensional row-major `[level][y][x]` full-level array.
    Full3D {
        /// Full-level count.
        levels: usize,
        /// Latitude or row count.
        ny: usize,
        /// Longitude or column count.
        nx: usize,
    },
    /// Three-dimensional row-major `[level][y][x]` interface-level array.
    Interface3D {
        /// Interface-level count.
        levels: usize,
        /// Latitude or row count.
        ny: usize,
        /// Longitude or column count.
        nx: usize,
    },
}

impl ArrayLayout {
    /// Returns the semantic field shape.
    #[must_use]
    pub const fn shape(self) -> FieldShape {
        match self {
            Self::Scalar => FieldShape::Scalar,
            Self::Horizontal2D { .. } => FieldShape::Horizontal2D,
            Self::Full3D { .. } => FieldShape::Full3D,
            Self::Interface3D { .. } => FieldShape::Interface3D,
        }
    }

    /// Returns the checked flat element count.
    pub fn element_count(self) -> Result<usize, FrameError> {
        match self {
            Self::Scalar => Ok(1),
            Self::Horizontal2D { ny, nx } => checked_grid_count(nx, ny),
            Self::Full3D { levels, ny, nx } | Self::Interface3D { levels, ny, nx } => {
                if levels == 0 {
                    return Err(FrameError::InvalidLayout(
                        "three-dimensional level count must be positive".into(),
                    ));
                }
                checked_grid_count(nx, ny)?
                    .checked_mul(levels)
                    .ok_or_else(|| FrameError::InvalidLayout("array element count overflow".into()))
            }
        }
    }

    /// Returns horizontal dimensions when the value is gridded.
    #[must_use]
    pub const fn horizontal_dimensions(self) -> Option<(usize, usize)> {
        match self {
            Self::Scalar => None,
            Self::Horizontal2D { ny, nx }
            | Self::Full3D { ny, nx, .. }
            | Self::Interface3D { ny, nx, .. } => Some((ny, nx)),
        }
    }

    /// Returns the vertical count for three-dimensional arrays.
    #[must_use]
    pub const fn level_count(self) -> Option<usize> {
        match self {
            Self::Full3D { levels, .. } | Self::Interface3D { levels, .. } => Some(levels),
            Self::Scalar | Self::Horizontal2D { .. } => None,
        }
    }
}

fn checked_grid_count(nx: usize, ny: usize) -> Result<usize, FrameError> {
    if nx == 0 || ny == 0 {
        return Err(FrameError::InvalidLayout(
            "horizontal dimensions must be positive".into(),
        ));
    }
    nx.checked_mul(ny)
        .ok_or_else(|| FrameError::InvalidLayout("array element count overflow".into()))
}

/// Explicit source-time support for one field value array.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum TemporalSupport {
    /// Value valid at one instant.
    Instantaneous {
        /// Physical validity time.
        valid_time: Timestamp,
    },
    /// Mean or integral over a closed-open interval ending at frame time.
    Interval {
        /// Inclusive interval start.
        start: Timestamp,
        /// Exclusive interval end and frame validity time.
        end: Timestamp,
    },
    /// Accumulation from an explicit reset origin through frame time.
    Accumulation {
        /// Accumulation reset origin.
        reset: Timestamp,
        /// Inclusive support start used by this decoded value.
        start: Timestamp,
        /// Exclusive support end and frame validity time.
        end: Timestamp,
    },
}

impl TemporalSupport {
    /// Returns the physical frame time represented by this support.
    #[must_use]
    pub const fn valid_time(self) -> Timestamp {
        match self {
            Self::Instantaneous { valid_time } => valid_time,
            Self::Interval { end, .. } | Self::Accumulation { end, .. } => end,
        }
    }

    fn validate(self) -> Result<(), FrameError> {
        match self {
            Self::Instantaneous { .. } => Ok(()),
            Self::Interval { start, end } if start < end => Ok(()),
            Self::Accumulation { reset, start, end } if reset <= start && start < end => Ok(()),
            Self::Interval { .. } => Err(FrameError::InvalidTemporalSupport(
                "interval support requires start < end".into(),
            )),
            Self::Accumulation { .. } => Err(FrameError::InvalidTemporalSupport(
                "accumulation support requires reset <= start < end".into(),
            )),
        }
    }
}

/// Independent structural validity mask.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValidityMask {
    valid: Arc<[bool]>,
}

impl ValidityMask {
    /// Creates a mask with one entry per value.
    #[must_use]
    pub fn new(valid: Arc<[bool]>) -> Self {
        Self { valid }
    }

    /// Returns mask values in canonical array order.
    #[must_use]
    pub const fn as_arc(&self) -> &Arc<[bool]> {
        &self.valid
    }

    /// Returns whether one flat element is structurally valid.
    #[must_use]
    pub fn get(&self, index: usize) -> Option<bool> {
        self.valid.get(index).copied()
    }

    /// Returns the mask length.
    #[must_use]
    pub fn len(&self) -> usize {
        self.valid.len()
    }

    /// Returns whether the mask is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.valid.is_empty()
    }
}

/// One immutable normalized meteorological field.
#[derive(Clone, Debug, PartialEq)]
pub struct RawField {
    values: Arc<[f64]>,
    validity: ValidityMask,
    unit: GraphUnit,
    layout: ArrayLayout,
    temporal: TemporalSupport,
    quality: FieldQuality,
    provenance: ProvenanceId,
}

impl RawField {
    /// Validates and constructs a normalized immutable field.
    pub fn new(
        values: Arc<[f64]>,
        valid: Arc<[bool]>,
        unit: GraphUnit,
        layout: ArrayLayout,
        temporal: TemporalSupport,
        quality: FieldQuality,
        provenance: ProvenanceId,
    ) -> Result<Self, FrameError> {
        let expected = layout.element_count()?;
        if values.len() != expected || valid.len() != expected {
            return Err(FrameError::FieldLengthMismatch {
                expected,
                values: values.len(),
                validity: valid.len(),
            });
        }
        if values.iter().any(|value| !value.is_finite()) {
            return Err(FrameError::NonFiniteFieldValue);
        }
        temporal.validate()?;
        Ok(Self {
            values,
            validity: ValidityMask::new(valid),
            unit,
            layout,
            temporal,
            quality,
            provenance,
        })
    }

    /// Returns finite values in canonical order.
    #[must_use]
    pub const fn values(&self) -> &Arc<[f64]> {
        &self.values
    }

    /// Returns the independent validity mask.
    #[must_use]
    pub const fn validity(&self) -> &ValidityMask {
        &self.validity
    }

    /// Returns the normalized unit.
    #[must_use]
    pub const fn unit(&self) -> &GraphUnit {
        &self.unit
    }

    /// Returns canonical array layout.
    #[must_use]
    pub const fn layout(&self) -> ArrayLayout {
        self.layout
    }

    /// Returns physical time support.
    #[must_use]
    pub const fn temporal(&self) -> TemporalSupport {
        self.temporal
    }

    /// Returns source, derived, or estimated quality.
    #[must_use]
    pub const fn quality(&self) -> FieldQuality {
        self.quality
    }

    /// Returns the compact provenance record identifier.
    #[must_use]
    pub const fn provenance(&self) -> ProvenanceId {
        self.provenance
    }

    /// Returns resident value and mask bytes, excluding shared metadata.
    #[must_use]
    pub fn resident_bytes(&self) -> u64 {
        let values = self.values.len().saturating_mul(std::mem::size_of::<f64>());
        let mask = self
            .validity
            .len()
            .saturating_mul(std::mem::size_of::<bool>());
        u64::try_from(values.saturating_add(mask)).unwrap_or(u64::MAX)
    }
}

/// Structure-of-arrays storage for immutable decoded source fields.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RawFieldStore {
    fields: BTreeMap<FieldKey, RawField>,
}

impl RawFieldStore {
    /// Creates an empty store during frame assembly.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            fields: BTreeMap::new(),
        }
    }

    /// Inserts one unique decoded field before frame publication.
    pub fn insert(&mut self, key: FieldKey, field: RawField) -> Result<(), FrameError> {
        if self.fields.contains_key(&key) {
            return Err(FrameError::DuplicateField(key));
        }
        self.fields.insert(key, field);
        Ok(())
    }

    /// Returns an immutable field.
    #[must_use]
    pub fn get(&self, key: &FieldKey) -> Option<&RawField> {
        self.fields.get(key)
    }

    /// Iterates fields in stable key order.
    pub fn iter(&self) -> impl Iterator<Item = (&FieldKey, &RawField)> {
        self.fields.iter()
    }

    /// Returns the number of published fields.
    #[must_use]
    pub fn len(&self) -> usize {
        self.fields.len()
    }

    /// Returns whether no fields have been assembled.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.fields.is_empty()
    }

    /// Returns total resident value and mask bytes.
    #[must_use]
    pub fn resident_bytes(&self) -> u64 {
        self.fields.values().fold(0_u64, |total, field| {
            total.saturating_add(field.resident_bytes())
        })
    }
}

/// Provenance-free array value used while executing the Profile Frame graph.
#[derive(Clone, Debug, PartialEq)]
pub struct FrameComputationField {
    /// Finite values in canonical array order.
    pub values: Arc<[f64]>,
    /// Independent structural and mathematical validity mask.
    pub valid: Arc<[bool]>,
    /// Physical unit of the values.
    pub unit: GraphUnit,
    /// Canonical runtime layout.
    pub layout: ArrayLayout,
    /// Physical source-time support.
    pub temporal: TemporalSupport,
}

impl FrameComputationField {
    /// Validates and creates one executor value.
    pub fn new(
        values: Arc<[f64]>,
        valid: Arc<[bool]>,
        unit: GraphUnit,
        layout: ArrayLayout,
        temporal: TemporalSupport,
    ) -> Result<Self, FrameGraphExecutionError> {
        let expected = layout
            .element_count()
            .map_err(|error| FrameGraphExecutionError::InvalidValue(format!("{error:?}")))?;
        if values.len() != expected || valid.len() != expected {
            return Err(FrameGraphExecutionError::InvalidValue(
                "executor value length disagrees with its layout".into(),
            ));
        }
        if values.iter().any(|value| !value.is_finite()) {
            return Err(FrameGraphExecutionError::InvalidValue(
                "executor values must be finite".into(),
            ));
        }
        temporal
            .validate()
            .map_err(|error| FrameGraphExecutionError::InvalidValue(format!("{error:?}")))?;
        Ok(Self {
            values,
            valid,
            unit,
            layout,
            temporal,
        })
    }
}

/// Complete immutable input to one bounded Frame-stage graph execution.
#[derive(Clone, Copy, Debug)]
pub struct FrameGraphExecutionRequest<'a> {
    /// Validated computation graph.
    pub graph: &'a ComputationGraph,
    /// Deterministic compiled stage order for `graph`.
    pub execution_plan: &'a ExecutionPlan,
    /// Canonical outputs required by the active capabilities.
    pub requested_outputs: &'a BTreeSet<FieldKey>,
    /// Unit-normalized current-frame direct source values.
    pub sources: &'a BTreeMap<FieldKey, FrameComputationField>,
    /// Optional previous-frame direct values used by de-accumulation.
    pub previous_sources: Option<&'a BTreeMap<FieldKey, FrameComputationField>>,
    /// Physical time assigned to constants and interval scalars.
    pub valid_time: Timestamp,
    /// Explicit interval duration available to `interval_seconds()`.
    pub interval_seconds: Option<f64>,
}

/// Deterministic, side-effect-free executor for Profile Frame-stage nodes.
#[derive(Clone, Copy, Debug, Default)]
pub struct FrameGraphExecutor;

impl FrameGraphExecutor {
    /// Executes only graph nodes reachable from the requested Frame outputs.
    pub fn execute(
        request: FrameGraphExecutionRequest<'_>,
    ) -> Result<BTreeMap<FieldKey, FrameComputationField>, FrameGraphExecutionError> {
        let nodes = request
            .graph
            .nodes
            .iter()
            .map(|node| (node.id.clone(), node))
            .collect::<BTreeMap<_, _>>();
        let output_nodes = request
            .graph
            .outputs
            .iter()
            .filter(|(field, _)| request.requested_outputs.contains(field))
            .cloned()
            .collect::<Vec<_>>();
        if output_nodes.len() != request.requested_outputs.len() {
            return Err(FrameGraphExecutionError::MissingOutput);
        }
        let needed = reachable_nodes(&output_nodes, &nodes)?;
        for id in &needed {
            let node = nodes
                .get(id)
                .ok_or_else(|| FrameGraphExecutionError::MissingNode(id.clone()))?;
            if node.stage != ExecutionStage::Frame {
                return Err(FrameGraphExecutionError::UnsupportedStage {
                    node: id.clone(),
                    stage: node.stage,
                });
            }
        }

        let frame_stage = request
            .execution_plan
            .stages
            .iter()
            .find(|stage| stage.stage == ExecutionStage::Frame);
        let mut values = BTreeMap::<GraphNodeId, FrameComputationField>::new();
        if let Some(stage) = frame_stage {
            for id in &stage.nodes {
                if !needed.contains(id) {
                    continue;
                }
                let node = nodes
                    .get(id)
                    .ok_or_else(|| FrameGraphExecutionError::MissingNode(id.clone()))?;
                let value = evaluate_frame_node(node, &nodes, &values, request)?;
                values.insert(id.clone(), value);
            }
        }

        output_nodes
            .into_iter()
            .map(|(field, id)| {
                values
                    .get(&id)
                    .cloned()
                    .map(|value| (field, value))
                    .ok_or(FrameGraphExecutionError::MissingValue(id))
            })
            .collect()
    }
}

fn reachable_nodes(
    outputs: &[(FieldKey, GraphNodeId)],
    nodes: &BTreeMap<GraphNodeId, &GraphNode>,
) -> Result<BTreeSet<GraphNodeId>, FrameGraphExecutionError> {
    let mut needed = BTreeSet::new();
    let mut pending = outputs.iter().map(|(_, id)| id.clone()).collect::<Vec<_>>();
    while let Some(id) = pending.pop() {
        if !needed.insert(id.clone()) {
            continue;
        }
        let node = nodes
            .get(&id)
            .ok_or_else(|| FrameGraphExecutionError::MissingNode(id.clone()))?;
        pending.extend(node.inputs.iter().cloned());
    }
    Ok(needed)
}

fn evaluate_frame_node(
    node: &GraphNode,
    nodes: &BTreeMap<GraphNodeId, &GraphNode>,
    values: &BTreeMap<GraphNodeId, FrameComputationField>,
    request: FrameGraphExecutionRequest<'_>,
) -> Result<FrameComputationField, FrameGraphExecutionError> {
    let inputs = node
        .inputs
        .iter()
        .map(|id| {
            values
                .get(id)
                .ok_or_else(|| FrameGraphExecutionError::MissingValue(id.clone()))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let value = match &node.operation {
        GraphOp::Source(field) => request
            .sources
            .get(field)
            .cloned()
            .ok_or_else(|| FrameGraphExecutionError::MissingSource(field.clone()))?,
        GraphOp::Constant(value) => {
            scalar_field(*value, node.output_type.unit.clone(), request.valid_time)?
        }
        GraphOp::IntervalSeconds => scalar_field(
            request
                .interval_seconds
                .ok_or(FrameGraphExecutionError::MissingInterval)?,
            node.output_type.unit.clone(),
            request.valid_time,
        )?,
        GraphOp::Alias => inputs[0].clone(),
        GraphOp::ConvertUnit(target) => convert_field(inputs[0], target)?,
        GraphOp::Negate => unary_field(inputs[0], &node.output_type.unit, |value| -value)?,
        GraphOp::Add => binary_field(
            inputs[0],
            inputs[1],
            &node.output_type.unit,
            BinaryFrameOp::Add,
        )?,
        GraphOp::Subtract => binary_field(
            inputs[0],
            inputs[1],
            &node.output_type.unit,
            BinaryFrameOp::Subtract,
        )?,
        GraphOp::Multiply => binary_field(
            inputs[0],
            inputs[1],
            &node.output_type.unit,
            BinaryFrameOp::Multiply,
        )?,
        GraphOp::Divide => binary_field(
            inputs[0],
            inputs[1],
            &node.output_type.unit,
            BinaryFrameOp::Divide,
        )?,
        GraphOp::Deaccumulate => {
            let input_id = node
                .inputs
                .first()
                .ok_or_else(|| FrameGraphExecutionError::MissingValue(node.id.clone()))?;
            let input_node = nodes
                .get(input_id)
                .ok_or_else(|| FrameGraphExecutionError::MissingNode(input_id.clone()))?;
            let GraphOp::Source(field) = &input_node.operation else {
                return Err(FrameGraphExecutionError::UnsupportedDeaccumulation(
                    node.id.clone(),
                ));
            };
            let previous = request
                .previous_sources
                .and_then(|sources| sources.get(field))
                .ok_or_else(|| FrameGraphExecutionError::MissingPreviousSource(field.clone()))?;
            binary_field(
                inputs[0],
                previous,
                &node.output_type.unit,
                BinaryFrameOp::Subtract,
            )?
        }
        GraphOp::Thermodynamic(operation) => {
            thermodynamic_field(*operation, &inputs, &node.output_type.unit)?
        }
        GraphOp::Vertical(_) => {
            return Err(FrameGraphExecutionError::UnsupportedStage {
                node: node.id.clone(),
                stage: ExecutionStage::Column,
            });
        }
    };
    validate_node_value(node, value)
}

fn validate_node_value(
    node: &GraphNode,
    value: FrameComputationField,
) -> Result<FrameComputationField, FrameGraphExecutionError> {
    if value.layout.shape() != node.output_type.shape
        || !same_unit_conversion(&value.unit, &node.output_type.unit)
    {
        return Err(FrameGraphExecutionError::TypeMismatch(node.id.clone()));
    }
    Ok(value)
}

fn scalar_field(
    value: f64,
    unit: GraphUnit,
    valid_time: Timestamp,
) -> Result<FrameComputationField, FrameGraphExecutionError> {
    FrameComputationField::new(
        Arc::from([value]),
        Arc::from([value.is_finite()]),
        unit,
        ArrayLayout::Scalar,
        TemporalSupport::Instantaneous { valid_time },
    )
}

fn convert_field(
    input: &FrameComputationField,
    target: &GraphUnit,
) -> Result<FrameComputationField, FrameGraphExecutionError> {
    let converted = input
        .values
        .iter()
        .zip(input.valid.iter())
        .map(|(value, valid)| {
            if *valid {
                input
                    .unit
                    .convert_value_to(*value, target)
                    .map_err(|error| FrameGraphExecutionError::InvalidValue(error.to_string()))
            } else {
                Ok(0.0)
            }
        })
        .collect::<Result<Vec<_>, _>>()?;
    FrameComputationField::new(
        Arc::from(converted),
        input.valid.clone(),
        target.clone(),
        input.layout,
        input.temporal,
    )
}

fn unary_field(
    input: &FrameComputationField,
    output_unit: &GraphUnit,
    operation: impl Fn(f64) -> f64,
) -> Result<FrameComputationField, FrameGraphExecutionError> {
    let mut output_valid = input.valid.to_vec();
    let output = input
        .values
        .iter()
        .zip(output_valid.iter_mut())
        .map(|(value, valid)| {
            if !*valid {
                return 0.0;
            }
            let result = operation(*value);
            if result.is_finite() {
                result
            } else {
                *valid = false;
                0.0
            }
        })
        .collect::<Vec<_>>();
    FrameComputationField::new(
        Arc::from(output),
        Arc::from(output_valid),
        output_unit.clone(),
        input.layout,
        input.temporal,
    )
}

#[derive(Clone, Copy)]
enum BinaryFrameOp {
    Add,
    Subtract,
    Multiply,
    Divide,
}

fn binary_field(
    left: &FrameComputationField,
    right: &FrameComputationField,
    output_unit: &GraphUnit,
    operation: BinaryFrameOp,
) -> Result<FrameComputationField, FrameGraphExecutionError> {
    let layout = broadcast_layout(left.layout, right.layout)?;
    let count = layout
        .element_count()
        .map_err(|error| FrameGraphExecutionError::InvalidValue(format!("{error:?}")))?;
    let temporal = broadcast_temporal(left, right)?;
    let mut output = Vec::with_capacity(count);
    let mut output_valid = Vec::with_capacity(count);
    for index in 0..count {
        let left_index = if left.layout == ArrayLayout::Scalar {
            0
        } else {
            index
        };
        let right_index = if right.layout == ArrayLayout::Scalar {
            0
        } else {
            index
        };
        let valid = left.valid[left_index] && right.valid[right_index];
        if !valid {
            output.push(0.0);
            output_valid.push(false);
            continue;
        }
        let left_value = left.values[left_index];
        let right_value = right.values[right_index];
        let result = match operation {
            BinaryFrameOp::Add => left_value + right_value,
            BinaryFrameOp::Subtract => left_value - right_value,
            BinaryFrameOp::Multiply => {
                let si = (left_value * left.unit.scale_to_si())
                    * (right_value * right.unit.scale_to_si());
                si / output_unit.scale_to_si()
            }
            BinaryFrameOp::Divide if right_value != 0.0 => {
                let si = (left_value * left.unit.scale_to_si())
                    / (right_value * right.unit.scale_to_si());
                si / output_unit.scale_to_si()
            }
            BinaryFrameOp::Divide => {
                output.push(0.0);
                output_valid.push(false);
                continue;
            }
        };
        if result.is_finite() {
            output.push(result);
            output_valid.push(true);
        } else {
            output.push(0.0);
            output_valid.push(false);
        }
    }
    FrameComputationField::new(
        Arc::from(output),
        Arc::from(output_valid),
        output_unit.clone(),
        layout,
        temporal,
    )
}

fn thermodynamic_field(
    operation: ThermodynamicOp,
    inputs: &[&FrameComputationField],
    output_unit: &GraphUnit,
) -> Result<FrameComputationField, FrameGraphExecutionError> {
    let layout = inputs
        .iter()
        .try_fold(ArrayLayout::Scalar, |layout, input| {
            broadcast_layout(layout, input.layout)
        })?;
    let count = layout
        .element_count()
        .map_err(|error| FrameGraphExecutionError::InvalidValue(format!("{error:?}")))?;
    let temporal = inputs
        .iter()
        .try_fold(None, |current: Option<TemporalSupport>, input| {
            let next = match current {
                Some(value) if value.valid_time() != input.temporal.valid_time() => {
                    return Err(FrameGraphExecutionError::TimeMismatch);
                }
                Some(value) => value,
                None => input.temporal,
            };
            Ok(Some(next))
        })?
        .ok_or(FrameGraphExecutionError::TimeMismatch)?;
    let mut output = Vec::with_capacity(count);
    let mut valid = Vec::with_capacity(count);
    for index in 0..count {
        let mut si = Vec::with_capacity(inputs.len());
        let mut supported = true;
        for input in inputs {
            let source_index = if input.layout == ArrayLayout::Scalar {
                0
            } else {
                index
            };
            supported &= input.valid[source_index];
            si.push(
                input.values[source_index]
                    .mul_add(input.unit.scale_to_si(), input.unit.offset_to_si()),
            );
        }
        let result_si = if supported {
            match operation {
                ThermodynamicOp::VirtualTemperature => si[0] * (1.0 + 0.61 * si[1]),
                ThermodynamicOp::AirDensity => si[0] / (287.05 * si[1]),
                ThermodynamicOp::RelativeHumidity => {
                    let saturation = 611.2 * (17.67 * (si[0] - 273.15) / (si[0] - 29.65)).exp();
                    let vapour = si[1] * si[2] / (0.622 + 0.378 * si[1]);
                    vapour / saturation
                }
                ThermodynamicOp::PotentialTemperature => {
                    si[0] * (100_000.0 / si[1]).powf(287.05 / 1_004.0)
                }
                ThermodynamicOp::GeopotentialHeight => si[0] / M3_CONSTANTS.standard_gravity_m_s2,
                ThermodynamicOp::GeopotentialFromHeight => {
                    si[0] * M3_CONSTANTS.standard_gravity_m_s2
                }
                ThermodynamicOp::SurfacePressureFromLog => si[0].exp(),
                ThermodynamicOp::TwoMetreSpecificHumidityFromDewpoint => {
                    two_metre_specific_humidity_from_dewpoint_si(si[0], si[1]).unwrap_or(f64::NAN)
                }
                ThermodynamicOp::LatentHeatFromMoistureFlux => {
                    // ECMWF moisture flux is positive downward; canonical LE is upward.
                    -M3_CONSTANTS.latent_heat_vaporization_j_kg * si[0]
                }
            }
        } else {
            0.0
        };
        let result = (result_si - output_unit.offset_to_si()) / output_unit.scale_to_si();
        if supported && result.is_finite() {
            output.push(result);
            valid.push(true);
        } else {
            output.push(0.0);
            valid.push(false);
        }
    }
    FrameComputationField::new(
        Arc::from(output),
        Arc::from(valid),
        output_unit.clone(),
        layout,
        temporal,
    )
}

fn broadcast_layout(
    left: ArrayLayout,
    right: ArrayLayout,
) -> Result<ArrayLayout, FrameGraphExecutionError> {
    match (left, right) {
        (ArrayLayout::Scalar, value) | (value, ArrayLayout::Scalar) => Ok(value),
        (left, right) if left == right => Ok(left),
        _ => Err(FrameGraphExecutionError::LayoutMismatch),
    }
}

fn broadcast_temporal(
    left: &FrameComputationField,
    right: &FrameComputationField,
) -> Result<TemporalSupport, FrameGraphExecutionError> {
    if left.temporal.valid_time() != right.temporal.valid_time() {
        return Err(FrameGraphExecutionError::TimeMismatch);
    }
    Ok(if left.layout == ArrayLayout::Scalar {
        right.temporal
    } else {
        left.temporal
    })
}

fn same_unit_conversion(left: &GraphUnit, right: &GraphUnit) -> bool {
    left.dimension() == right.dimension()
        && left.scale_to_si() == right.scale_to_si()
        && left.offset_to_si() == right.offset_to_si()
}

/// Frame-stage graph evaluation failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FrameGraphExecutionError {
    /// A requested graph output is not declared.
    MissingOutput,
    /// A referenced graph node is absent.
    MissingNode(GraphNodeId),
    /// A predecessor did not produce a runtime value.
    MissingValue(GraphNodeId),
    /// A required direct source field was not supplied.
    MissingSource(FieldKey),
    /// A de-accumulation source has no preceding-frame value.
    MissingPreviousSource(FieldKey),
    /// `interval_seconds()` has no explicit execution interval.
    MissingInterval,
    /// A reachable output requires a stage later than Frame.
    UnsupportedStage {
        /// Node requiring the later stage.
        node: GraphNodeId,
        /// Required stage.
        stage: ExecutionStage,
    },
    /// v0 de-accumulation only accepts a direct source input.
    UnsupportedDeaccumulation(GraphNodeId),
    /// Runtime units or shapes disagree with the compiled node type.
    TypeMismatch(GraphNodeId),
    /// Non-scalar runtime layouts cannot be broadcast.
    LayoutMismatch,
    /// Inputs represent different physical frame times.
    TimeMismatch,
    /// A runtime array or numeric operation is invalid.
    InvalidValue(String),
}

/// Identity and topology shared by all fields in one frame.
#[derive(Clone, Debug, PartialEq)]
pub struct FrameMetadata {
    /// Stable logical frame identity.
    pub id: LogicalFrameId,
    /// Selected domain.
    pub domain: DomainId,
    /// Physical validity time.
    pub valid_time: Timestamp,
    /// Queryable regular horizontal geometry.
    pub grid: DomainGeometry,
    /// Native vertical topology.
    pub vertical: VerticalTopology,
}

/// Immutable, fully assembled source and Frame-stage-derived field frame.
#[derive(Clone, Debug, PartialEq)]
pub struct RawMetFrame {
    metadata: FrameMetadata,
    fields: RawFieldStore,
    provenance: Arc<ProvenanceTable>,
}

impl RawMetFrame {
    /// Validates and publishes a completed immutable frame.
    pub fn publish(
        metadata: FrameMetadata,
        fields: RawFieldStore,
        provenance: Arc<ProvenanceTable>,
    ) -> Result<Self, FrameError> {
        validate_metadata(&metadata)?;
        if fields.is_empty() {
            return Err(FrameError::EmptyFrame);
        }
        let full_levels = full_level_count(&metadata.vertical)?;
        for (key, field) in fields.iter() {
            if field.temporal().valid_time() != metadata.valid_time {
                return Err(FrameError::FieldTimeMismatch(key.clone()));
            }
            if let Some((ny, nx)) = field.layout().horizontal_dimensions()
                && (nx != metadata.grid.nx || ny != metadata.grid.ny)
            {
                return Err(FrameError::HorizontalShapeMismatch(key.clone()));
            }
            match field.layout() {
                ArrayLayout::Full3D { levels, .. } if levels != full_levels => {
                    return Err(FrameError::VerticalShapeMismatch(key.clone()));
                }
                ArrayLayout::Interface3D { levels, .. }
                    if levels != full_levels.saturating_add(1) =>
                {
                    return Err(FrameError::VerticalShapeMismatch(key.clone()));
                }
                _ => {}
            }
            let Some(record) = provenance.get(field.provenance()) else {
                return Err(FrameError::MissingProvenance(field.provenance()));
            };
            if &record.field != key
                || record.quality != field.quality()
                || record.profile_sha256 != metadata.id.profile_sha256
            {
                return Err(FrameError::ProvenanceMismatch(key.clone()));
            }
        }
        Ok(Self {
            metadata,
            fields,
            provenance,
        })
    }

    /// Returns immutable frame metadata.
    #[must_use]
    pub const fn metadata(&self) -> &FrameMetadata {
        &self.metadata
    }

    /// Returns immutable normalized fields.
    #[must_use]
    pub const fn fields(&self) -> &RawFieldStore {
        &self.fields
    }

    /// Returns the frame-level deduplicated provenance table.
    #[must_use]
    pub const fn provenance(&self) -> &Arc<ProvenanceTable> {
        &self.provenance
    }

    /// Returns resident field value and mask bytes.
    #[must_use]
    pub fn resident_bytes(&self) -> u64 {
        self.fields.resident_bytes()
    }
}

fn validate_metadata(metadata: &FrameMetadata) -> Result<(), FrameError> {
    if metadata.id.domain != metadata.domain
        || metadata.id.valid_time != metadata.valid_time
        || metadata.grid.domain != metadata.domain
    {
        return Err(FrameError::MetadataIdentityMismatch);
    }
    if metadata.grid.nx == 0
        || metadata.grid.ny == 0
        || !metadata.grid.longitude_origin_degrees.is_finite()
        || !metadata.grid.latitude_origin_degrees.is_finite()
        || !metadata.grid.longitude_spacing_degrees.is_finite()
        || metadata.grid.longitude_spacing_degrees <= 0.0
        || !metadata.grid.latitude_spacing_degrees.is_finite()
        || metadata.grid.latitude_spacing_degrees == 0.0
    {
        return Err(FrameError::InvalidGridGeometry);
    }
    let _ = full_level_count(&metadata.vertical)?;
    Ok(())
}

fn full_level_count(vertical: &VerticalTopology) -> Result<usize, FrameError> {
    match vertical {
        VerticalTopology::HybridPressure(topology) => {
            let coefficients = &topology.coefficients;
            if coefficients.a_half_pa.len() < 2
                || coefficients.a_half_pa.len() != coefficients.b_half.len()
                || coefficients
                    .a_half_pa
                    .iter()
                    .chain(coefficients.b_half.iter())
                    .any(|value| !value.is_finite())
            {
                return Err(FrameError::InvalidVerticalTopology);
            }
            let complete_full_levels = coefficients.a_half_pa.len() - 1;
            if topology.active_full_levels.is_empty()
                || topology
                    .active_full_levels
                    .iter()
                    .any(|level| usize::from(*level) > complete_full_levels || *level == 0)
                || topology
                    .active_full_levels
                    .windows(2)
                    .any(|levels| levels[1] != levels[0].saturating_add(1))
            {
                return Err(FrameError::InvalidVerticalTopology);
            }
            Ok(topology.active_full_levels.len())
        }
        VerticalTopology::PressureLevels(levels) => {
            if levels.validate().is_err() {
                return Err(FrameError::InvalidVerticalTopology);
            }
            Ok(levels.pressure_pa.len())
        }
    }
}

/// Two frames that bracket one physical query time.
#[derive(Clone, Debug, PartialEq)]
pub struct FramePair {
    /// Earlier physical frame.
    pub before: Arc<RawMetFrame>,
    /// Later physical frame.
    pub after: Arc<RawMetFrame>,
}

/// Pinned frame pair and deterministic temporal weights.
#[derive(Clone, Debug)]
pub struct PreparedWindow {
    /// Frames from one selected domain.
    pub frames: FramePair,
    /// Previous physical frame for a symmetric exact-frame time derivative.
    pub previous: Option<Arc<RawMetFrame>>,
    /// Next physical frame for a symmetric exact-frame time derivative.
    pub next: Option<Arc<RawMetFrame>>,
    /// Exact physical query time.
    pub query_time: Timestamp,
    /// Frozen bytes available to batch-local columns, workspace, and chunks.
    pub execution_budget_bytes: u64,
    /// Weight assigned to the earlier frame.
    pub before_weight: f64,
    /// Weight assigned to the later frame.
    pub after_weight: f64,
    column_cache: Option<Weak<Mutex<ColumnCache>>>,
}

impl PartialEq for PreparedWindow {
    fn eq(&self, other: &Self) -> bool {
        self.frames == other.frames
            && self.previous == other.previous
            && self.next == other.next
            && self.query_time == other.query_time
            && self.execution_budget_bytes == other.execution_budget_bytes
            && self.before_weight == other.before_weight
            && self.after_weight == other.after_weight
    }
}

impl PreparedWindow {
    /// Constructs a strict between-frame window with linear time weights.
    pub fn between(
        before: Arc<RawMetFrame>,
        after: Arc<RawMetFrame>,
        query_time: Timestamp,
        execution_budget_bytes: u64,
    ) -> Result<Self, FrameError> {
        validate_frame_compatibility(&before, &after)?;
        let before_time = before.metadata.valid_time;
        let after_time = after.metadata.valid_time;
        if before_time >= after_time || query_time <= before_time || query_time >= after_time {
            return Err(FrameError::InvalidTimeWindow);
        }
        let total = timestamp_nanoseconds(after_time) - timestamp_nanoseconds(before_time);
        let elapsed = timestamp_nanoseconds(query_time) - timestamp_nanoseconds(before_time);
        let after_weight = elapsed as f64 / total as f64;
        let before_weight = 1.0 - after_weight;
        if !before_weight.is_finite()
            || !after_weight.is_finite()
            || before_weight <= 0.0
            || after_weight <= 0.0
        {
            return Err(FrameError::InvalidTimeWindow);
        }
        Ok(Self {
            frames: FramePair { before, after },
            previous: None,
            next: None,
            query_time,
            execution_budget_bytes,
            before_weight,
            after_weight,
            column_cache: None,
        })
    }

    /// Constructs an exact-frame window and retains adjacent derivative frames.
    pub fn at_frame(
        previous: Option<Arc<RawMetFrame>>,
        current: Arc<RawMetFrame>,
        next: Option<Arc<RawMetFrame>>,
        execution_budget_bytes: u64,
    ) -> Result<Self, FrameError> {
        if let Some(frame) = &previous {
            validate_frame_compatibility(frame, &current)?;
            if frame.metadata.valid_time >= current.metadata.valid_time {
                return Err(FrameError::InvalidTimeWindow);
            }
        }
        if let Some(frame) = &next {
            validate_frame_compatibility(&current, frame)?;
            if frame.metadata.valid_time <= current.metadata.valid_time {
                return Err(FrameError::InvalidTimeWindow);
            }
        }
        let query_time = current.metadata.valid_time;
        Ok(Self {
            frames: FramePair {
                before: current.clone(),
                after: current,
            },
            previous,
            next,
            query_time,
            execution_budget_bytes,
            before_weight: 1.0,
            after_weight: 0.0,
            column_cache: None,
        })
    }

    /// Returns whether this query lies exactly on one physical frame.
    #[must_use]
    pub fn is_exact_frame(&self) -> bool {
        self.frames.before.metadata.valid_time == self.frames.after.metadata.valid_time
    }

    /// Verifies the extra frame support required by complete geometric W.
    pub fn validate_transport_time_support(&self) -> Result<(), FrameError> {
        if self.is_exact_frame() && (self.previous.is_none() || self.next.is_none()) {
            return Err(FrameError::MissingSymmetricTimeSupport);
        }
        Ok(())
    }

    /// Attaches the engine-owned preparation cache without retaining the engine.
    pub(crate) fn attach_column_cache(&mut self, cache: &Arc<Mutex<ColumnCache>>) {
        self.column_cache = Some(Arc::downgrade(cache));
    }

    /// Upgrades the engine-owned cache while preparing a batch.
    pub(crate) fn column_cache(&self) -> Option<Arc<Mutex<ColumnCache>>> {
        self.column_cache.as_ref().and_then(Weak::upgrade)
    }
}

fn validate_frame_compatibility(left: &RawMetFrame, right: &RawMetFrame) -> Result<(), FrameError> {
    if left.metadata.domain != right.metadata.domain
        || left.metadata.grid != right.metadata.grid
        || left.metadata.vertical != right.metadata.vertical
    {
        return Err(FrameError::IncompatibleWindowFrames);
    }
    Ok(())
}

fn timestamp_nanoseconds(time: Timestamp) -> i128 {
    i128::from(time.seconds_since_unix_epoch()) * 1_000_000_000_i128 + i128::from(time.nanosecond())
}

/// Mutable owner of frame transitions and prefetch policy.
#[derive(Clone, Debug, Default)]
pub struct WindowManager;

impl WindowManager {
    /// Selects a strict window from frames sorted by increasing physical time.
    pub fn prepare_from_sorted_frames(
        &mut self,
        time: Timestamp,
        frames: &[Arc<RawMetFrame>],
        execution_budget_bytes: u64,
    ) -> Result<PreparedWindow, FrameError> {
        if frames.is_empty()
            || frames
                .windows(2)
                .any(|pair| pair[1].metadata.valid_time <= pair[0].metadata.valid_time)
        {
            return Err(FrameError::InvalidFrameSequence);
        }
        match frames.binary_search_by_key(&time, |frame| frame.metadata.valid_time) {
            Ok(index) => PreparedWindow::at_frame(
                index.checked_sub(1).map(|value| frames[value].clone()),
                frames[index].clone(),
                frames.get(index + 1).cloned(),
                execution_budget_bytes,
            ),
            Err(0) => Err(FrameError::MissingBracketingFrames),
            Err(index) if index == frames.len() => Err(FrameError::MissingBracketingFrames),
            Err(index) => PreparedWindow::between(
                frames[index - 1].clone(),
                frames[index].clone(),
                time,
                execution_budget_bytes,
            ),
        }
    }
}

/// Observable raw-frame cache counters.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct FrameCacheMetrics {
    /// Successful identity lookups.
    pub hits: u64,
    /// Missing identity lookups.
    pub misses: u64,
    /// Entries removed to satisfy the hard budget.
    pub evictions: u64,
    /// Bytes currently owned by the cache map.
    pub resident_bytes: u64,
}

#[derive(Clone, Debug)]
struct FrameCacheEntry {
    frame: Arc<RawMetFrame>,
    size_bytes: u64,
    last_access: u64,
}

/// Byte-budgeted deterministic immutable raw-frame cache.
#[derive(Debug, Default)]
pub struct FrameCache {
    budget_bytes: u64,
    entries: BTreeMap<LogicalFrameId, FrameCacheEntry>,
    access_clock: u64,
    metrics: FrameCacheMetrics,
}

impl FrameCache {
    /// Creates an empty cache with a hard resident-byte budget.
    #[must_use]
    pub const fn new(budget_bytes: u64) -> Self {
        Self {
            budget_bytes,
            entries: BTreeMap::new(),
            access_clock: 0,
            metrics: FrameCacheMetrics {
                hits: 0,
                misses: 0,
                evictions: 0,
                resident_bytes: 0,
            },
        }
    }

    /// Returns the configured hard budget.
    #[must_use]
    pub const fn budget_bytes(&self) -> u64 {
        self.budget_bytes
    }

    /// Returns observable cache counters.
    #[must_use]
    pub const fn metrics(&self) -> FrameCacheMetrics {
        self.metrics
    }

    /// Returns the number of resident frame identities.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Returns whether no frames are resident.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Looks up and pins one frame by returning a shared strong reference.
    pub fn get(&mut self, id: &LogicalFrameId) -> Option<Arc<RawMetFrame>> {
        self.access_clock = self.access_clock.saturating_add(1);
        if let Some(entry) = self.entries.get_mut(id) {
            entry.last_access = self.access_clock;
            self.metrics.hits = self.metrics.hits.saturating_add(1);
            return Some(entry.frame.clone());
        }
        self.metrics.misses = self.metrics.misses.saturating_add(1);
        None
    }

    /// Inserts a measured immutable frame after planning all required evictions.
    pub fn insert(&mut self, frame: Arc<RawMetFrame>) -> Result<Arc<RawMetFrame>, FrameError> {
        let id = frame.metadata.id.clone();
        if let Some(existing) = self.entries.get(&id) {
            if existing.frame.as_ref() == frame.as_ref() {
                return Ok(existing.frame.clone());
            }
            return Err(FrameError::DuplicateFrameIdentity(id));
        }
        let size_bytes = frame.resident_bytes();
        if size_bytes > self.budget_bytes {
            return Err(FrameError::InsufficientMemory {
                required_bytes: size_bytes,
                available_bytes: self.budget_bytes,
            });
        }
        let required = self.metrics.resident_bytes.checked_add(size_bytes).ok_or(
            FrameError::InsufficientMemory {
                required_bytes: u64::MAX,
                available_bytes: self.budget_bytes,
            },
        )?;
        let mut candidates = self
            .entries
            .iter()
            .filter(|(_, entry)| Arc::strong_count(&entry.frame) == 1)
            .map(|(key, entry)| (entry.last_access, key.clone(), entry.size_bytes))
            .collect::<Vec<_>>();
        candidates.sort_by(|left, right| (left.0, &left.1).cmp(&(right.0, &right.1)));
        let mut projected = required;
        let mut evict = Vec::new();
        for (_, key, bytes) in candidates {
            if projected <= self.budget_bytes {
                break;
            }
            projected = projected.saturating_sub(bytes);
            evict.push(key);
        }
        if projected > self.budget_bytes {
            return Err(FrameError::InsufficientMemory {
                required_bytes: required,
                available_bytes: self.budget_bytes,
            });
        }
        for key in evict {
            if let Some(entry) = self.entries.remove(&key) {
                self.metrics.resident_bytes =
                    self.metrics.resident_bytes.saturating_sub(entry.size_bytes);
                self.metrics.evictions = self.metrics.evictions.saturating_add(1);
            }
        }
        self.access_clock = self.access_clock.saturating_add(1);
        self.metrics.resident_bytes = self.metrics.resident_bytes.saturating_add(size_bytes);
        self.entries.insert(
            id,
            FrameCacheEntry {
                frame: frame.clone(),
                size_bytes,
                last_access: self.access_clock,
            },
        );
        Ok(frame)
    }

    /// Removes one unpinned identity; pinned frames remain resident.
    pub fn remove(&mut self, id: &LogicalFrameId) -> Result<bool, FrameError> {
        let Some(entry) = self.entries.get(id) else {
            return Ok(false);
        };
        if Arc::strong_count(&entry.frame) > 1 {
            return Err(FrameError::PinnedFrame);
        }
        let entry = self.entries.remove(id).ok_or(FrameError::PinnedFrame)?;
        self.metrics.resident_bytes = self.metrics.resident_bytes.saturating_sub(entry.size_bytes);
        Ok(true)
    }

    /// Returns pinned resident frames for one domain in increasing time order.
    #[must_use]
    pub fn frames_for_domain(&self, domain: &DomainId) -> Vec<Arc<RawMetFrame>> {
        let mut frames = self
            .entries
            .values()
            .filter(|entry| &entry.frame.metadata.domain == domain)
            .map(|entry| entry.frame.clone())
            .collect::<Vec<_>>();
        frames.sort_by_key(|frame| frame.metadata.valid_time);
        frames
    }
}

/// Frame assembly, selection, or cache failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FrameError {
    /// Candidate frames are empty or not strictly ordered by physical time.
    InvalidFrameSequence,
    /// No real frames strictly bracket the requested time.
    MissingBracketingFrames,
    /// Frame times or interpolation weights do not form a valid window.
    InvalidTimeWindow,
    /// Window frames disagree in domain, horizontal grid, or vertical topology.
    IncompatibleWindowFrames,
    /// Exact-frame geometric W lacks both previous and next physical frames.
    MissingSymmetricTimeSupport,
    /// One logical identity was reused for different immutable content.
    DuplicateFrameIdentity(LogicalFrameId),
    /// An explicit removal targeted a frame retained by a prepared owner.
    PinnedFrame,
    /// A field key occurs twice during assembly.
    DuplicateField(FieldKey),
    /// No fields were assembled.
    EmptyFrame,
    /// Array layout is zero-sized or overflows.
    InvalidLayout(String),
    /// Value and mask lengths disagree with layout.
    FieldLengthMismatch {
        /// Expected flat element count.
        expected: usize,
        /// Actual value count.
        values: usize,
        /// Actual validity count.
        validity: usize,
    },
    /// A value array contains NaN or infinity.
    NonFiniteFieldValue,
    /// Temporal bounds are inconsistent.
    InvalidTemporalSupport(String),
    /// Field time support does not end at the frame time.
    FieldTimeMismatch(FieldKey),
    /// Field horizontal dimensions disagree with frame geometry.
    HorizontalShapeMismatch(FieldKey),
    /// Field vertical count disagrees with native topology.
    VerticalShapeMismatch(FieldKey),
    /// Frame, domain, time, and grid identities disagree.
    MetadataIdentityMismatch,
    /// Horizontal geometry is structurally invalid.
    InvalidGridGeometry,
    /// Native vertical topology is structurally invalid.
    InvalidVerticalTopology,
    /// Field references a provenance record absent from the table.
    MissingProvenance(ProvenanceId),
    /// Field and provenance semantics disagree.
    ProvenanceMismatch(FieldKey),
    /// No frame pair covers the physical query time.
    MissingCoverage(Timestamp),
    /// Bracketing frames come from different domains.
    MixedDomains,
    /// Hard frame-cache memory budget is insufficient.
    InsufficientMemory {
        /// Required bytes.
        required_bytes: u64,
        /// Configured bytes.
        available_bytes: u64,
    },
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use trajecta_case::resolver::sha256_hex;

    use crate::field::{CanonicalField, ExtensionFieldId};
    use crate::profile::graph::{GraphCompiler, GraphNode, GraphOp, ThermodynamicOp, ValueType};
    use crate::provenance::ProvenanceRecord;
    use crate::vertical::{HybridCoefficients, HybridPressureTopology, PressureLevels};

    use super::*;

    fn metadata() -> FrameMetadata {
        let domain = DomainId("global".into());
        let valid_time = Timestamp::new(10_800, 0).unwrap();
        FrameMetadata {
            id: LogicalFrameId {
                domain: domain.clone(),
                valid_time,
                profile_sha256: sha256_hex(b"profile"),
                content_sha256: sha256_hex(b"frame"),
            },
            domain: domain.clone(),
            valid_time,
            grid: DomainGeometry {
                domain,
                longitude_origin_degrees: 0.0,
                latitude_origin_degrees: 90.0,
                longitude_spacing_degrees: 1.0,
                latitude_spacing_degrees: -1.0,
                nx: 2,
                ny: 2,
                periodic_longitude: false,
                halo_cells: 1,
            },
            vertical: VerticalTopology::PressureLevels(PressureLevels {
                pressure_pa: Arc::from([90_000.0, 100_000.0]),
            }),
        }
    }

    fn minimal_frame_at(seconds: i64) -> Arc<RawMetFrame> {
        let mut metadata = metadata();
        let valid_time = Timestamp::new(seconds, 0).unwrap();
        metadata.valid_time = valid_time;
        metadata.id.valid_time = valid_time;
        metadata.id.content_sha256 = sha256_hex(seconds.to_string().as_bytes());
        let key = FieldKey::Canonical(CanonicalField::AirTemperature);
        let mut table = ProvenanceTable::new();
        let provenance = table
            .intern(ProvenanceRecord {
                field: key.clone(),
                quality: FieldQuality::Source,
                sources: vec![format!("frame:{seconds}")],
                transforms: Vec::new(),
                fallback_reason: None,
                profile_sha256: metadata.id.profile_sha256.clone(),
            })
            .unwrap();
        let field = RawField::new(
            Arc::from([280.0; 8]),
            Arc::from([true; 8]),
            GraphUnit::parse("K").unwrap(),
            ArrayLayout::Full3D {
                levels: 2,
                ny: 2,
                nx: 2,
            },
            TemporalSupport::Instantaneous { valid_time },
            FieldQuality::Source,
            provenance,
        )
        .unwrap();
        let mut fields = RawFieldStore::new();
        fields.insert(key, field).unwrap();
        Arc::new(RawMetFrame::publish(metadata, fields, Arc::new(table)).unwrap())
    }

    #[test]
    fn window_manager_strictly_brackets_and_freezes_time_weights() {
        let frames = [
            minimal_frame_at(0),
            minimal_frame_at(3_600),
            minimal_frame_at(7_200),
        ];
        let mut manager = WindowManager;
        let between = manager
            .prepare_from_sorted_frames(Timestamp::new(1_800, 0).unwrap(), &frames, 1_000_000)
            .unwrap();
        assert_eq!(between.before_weight, 0.5);
        assert_eq!(between.after_weight, 0.5);
        assert!(!between.is_exact_frame());
        assert!(between.validate_transport_time_support().is_ok());

        let exact = manager
            .prepare_from_sorted_frames(Timestamp::new(3_600, 0).unwrap(), &frames, 1_000_000)
            .unwrap();
        assert!(exact.is_exact_frame());
        assert!(exact.validate_transport_time_support().is_ok());
        assert_eq!(
            exact.previous.as_ref().unwrap().metadata.valid_time,
            Timestamp::UNIX_EPOCH
        );
        assert_eq!(
            exact.next.as_ref().unwrap().metadata.valid_time,
            Timestamp::new(7_200, 0).unwrap()
        );
    }

    #[test]
    fn endpoint_exact_frame_is_not_complete_transport_coverage() {
        let frames = [minimal_frame_at(0), minimal_frame_at(3_600)];
        let mut manager = WindowManager;
        let endpoint = manager
            .prepare_from_sorted_frames(Timestamp::UNIX_EPOCH, &frames, 1_000_000)
            .unwrap();
        assert_eq!(
            endpoint.validate_transport_time_support(),
            Err(FrameError::MissingSymmetricTimeSupport)
        );
        assert_eq!(
            manager.prepare_from_sorted_frames(Timestamp::new(-1, 0).unwrap(), &frames, 1_000_000,),
            Err(FrameError::MissingBracketingFrames)
        );
    }

    #[test]
    fn frame_cache_evicts_deterministically_and_never_evicts_pins() {
        let frame0 = minimal_frame_at(0);
        let frame1 = minimal_frame_at(3_600);
        let frame2 = minimal_frame_at(7_200);
        let id0 = frame0.metadata.id.clone();
        let id1 = frame1.metadata.id.clone();
        let id2 = frame2.metadata.id.clone();
        let size = frame0.resident_bytes();
        let mut cache = FrameCache::new(size * 2);
        drop(cache.insert(frame0).unwrap());
        drop(cache.insert(frame1).unwrap());

        let pin0 = cache.get(&id0).unwrap();
        drop(cache.insert(frame2).unwrap());
        assert!(cache.get(&id1).is_none());
        assert!(cache.get(&id2).is_some());
        assert_eq!(cache.remove(&id0), Err(FrameError::PinnedFrame));
        drop(pin0);
        assert_eq!(cache.remove(&id0), Ok(true));
        assert!(cache.metrics().resident_bytes <= cache.budget_bytes());
    }

    #[test]
    fn frame_cache_failed_insert_is_transactional_when_all_entries_are_pinned() {
        let frame0 = minimal_frame_at(0);
        let frame1 = minimal_frame_at(3_600);
        let id0 = frame0.metadata.id.clone();
        let size = frame0.resident_bytes();
        let mut cache = FrameCache::new(size);
        drop(cache.insert(frame0).unwrap());
        let pin = cache.get(&id0).unwrap();
        assert!(matches!(
            cache.insert(frame1),
            Err(FrameError::InsufficientMemory { .. })
        ));
        assert_eq!(cache.len(), 1);
        assert_eq!(cache.metrics().resident_bytes, size);
        drop(pin);
    }

    #[test]
    fn frame_publication_checks_mask_shape_time_and_provenance() {
        let metadata = metadata();
        let key = FieldKey::Canonical(CanonicalField::AirTemperature);
        let mut table = ProvenanceTable::new();
        let provenance = table
            .intern(ProvenanceRecord {
                field: key.clone(),
                quality: FieldQuality::Source,
                sources: vec!["met:frame.grib:param=130".into()],
                transforms: Vec::new(),
                fallback_reason: None,
                profile_sha256: metadata.id.profile_sha256.clone(),
            })
            .unwrap();
        let field = RawField::new(
            Arc::from([280.0; 8]),
            Arc::from([true; 8]),
            GraphUnit::parse("K").unwrap(),
            ArrayLayout::Full3D {
                levels: 2,
                ny: 2,
                nx: 2,
            },
            TemporalSupport::Instantaneous {
                valid_time: metadata.valid_time,
            },
            FieldQuality::Source,
            provenance,
        )
        .unwrap();
        let mut fields = RawFieldStore::new();
        fields.insert(key, field).unwrap();
        let frame = RawMetFrame::publish(metadata, fields, Arc::new(table)).unwrap();
        assert_eq!(frame.resident_bytes(), 72);
    }

    #[test]
    fn non_finite_values_and_magic_missing_values_are_rejected() {
        let result = RawField::new(
            Arc::from([f64::NAN]),
            Arc::from([false]),
            GraphUnit::parse("1").unwrap(),
            ArrayLayout::Scalar,
            TemporalSupport::Instantaneous {
                valid_time: Timestamp::UNIX_EPOCH,
            },
            FieldQuality::Source,
            ProvenanceId(0),
        );
        assert_eq!(result, Err(FrameError::NonFiniteFieldValue));
    }

    #[test]
    fn hybrid_frame_accepts_active_bottom_subset_without_truncating_pv() {
        let mut metadata = metadata();
        metadata.vertical = VerticalTopology::HybridPressure(HybridPressureTopology {
            coefficients: HybridCoefficients {
                a_half_pa: Arc::from(vec![0.0; 138]),
                b_half: Arc::from(vec![0.0; 138]),
            },
            active_full_levels: Arc::from((130_u16..=137).collect::<Vec<_>>()),
        });
        let key = FieldKey::Canonical(CanonicalField::AirTemperature);
        let mut table = ProvenanceTable::new();
        let provenance = table
            .intern(ProvenanceRecord {
                field: key.clone(),
                quality: FieldQuality::Source,
                sources: vec!["met:frame.grib:param=130".into()],
                transforms: Vec::new(),
                fallback_reason: None,
                profile_sha256: metadata.id.profile_sha256.clone(),
            })
            .unwrap();
        let field = RawField::new(
            Arc::from([280.0; 32]),
            Arc::from([true; 32]),
            GraphUnit::parse("K").unwrap(),
            ArrayLayout::Full3D {
                levels: 8,
                ny: 2,
                nx: 2,
            },
            TemporalSupport::Instantaneous {
                valid_time: metadata.valid_time,
            },
            FieldQuality::Source,
            provenance,
        )
        .unwrap();
        let mut fields = RawFieldStore::new();
        fields.insert(key, field).unwrap();

        let frame = RawMetFrame::publish(metadata, fields, Arc::new(table)).unwrap();
        let VerticalTopology::HybridPressure(topology) = &frame.metadata().vertical else {
            panic!("expected hybrid topology");
        };
        assert_eq!(topology.coefficients.a_half_pa.len(), 138);
        assert_eq!(
            topology.active_full_levels.as_ref(),
            &[130, 131, 132, 133, 134, 135, 136, 137]
        );
    }

    #[test]
    fn frame_graph_executor_propagates_masks_through_thermodynamics() {
        let temperature = FieldKey::Canonical(CanonicalField::AirTemperature);
        let humidity = FieldKey::Canonical(CanonicalField::SpecificHumidity);
        let output = FieldKey::Extension(ExtensionFieldId {
            namespace: "test".into(),
            name: "virtual_temperature".into(),
        });
        let horizontal = |unit: &str| {
            ValueType::new(
                GraphUnit::parse(unit).unwrap(),
                FieldShape::Horizontal2D,
                None,
            )
            .unwrap()
        };
        let graph = ComputationGraph {
            nodes: vec![
                GraphNode {
                    id: GraphNodeId("temperature".into()),
                    operation: GraphOp::Source(temperature.clone()),
                    inputs: Vec::new(),
                    output_type: horizontal("K"),
                    stage: ExecutionStage::Frame,
                },
                GraphNode {
                    id: GraphNodeId("humidity".into()),
                    operation: GraphOp::Source(humidity.clone()),
                    inputs: Vec::new(),
                    output_type: horizontal("1"),
                    stage: ExecutionStage::Frame,
                },
                GraphNode {
                    id: GraphNodeId("virtual_temperature".into()),
                    operation: GraphOp::Thermodynamic(ThermodynamicOp::VirtualTemperature),
                    inputs: vec![
                        GraphNodeId("temperature".into()),
                        GraphNodeId("humidity".into()),
                    ],
                    output_type: horizontal("K"),
                    stage: ExecutionStage::Frame,
                },
            ],
            outputs: vec![(output.clone(), GraphNodeId("virtual_temperature".into()))],
        };
        let plan = GraphCompiler::compile(&graph).unwrap();
        let temporal = TemporalSupport::Instantaneous {
            valid_time: Timestamp::UNIX_EPOCH,
        };
        let sources = BTreeMap::from([
            (
                temperature,
                FrameComputationField::new(
                    Arc::from([300.0, 280.0]),
                    Arc::from([true, true]),
                    GraphUnit::parse("K").unwrap(),
                    ArrayLayout::Horizontal2D { ny: 1, nx: 2 },
                    temporal,
                )
                .unwrap(),
            ),
            (
                humidity,
                FrameComputationField::new(
                    Arc::from([0.01, 0.02]),
                    Arc::from([true, false]),
                    GraphUnit::parse("1").unwrap(),
                    ArrayLayout::Horizontal2D { ny: 1, nx: 2 },
                    temporal,
                )
                .unwrap(),
            ),
        ]);
        let requested = BTreeSet::from([output.clone()]);
        let values = FrameGraphExecutor::execute(FrameGraphExecutionRequest {
            graph: &graph,
            execution_plan: &plan,
            requested_outputs: &requested,
            sources: &sources,
            previous_sources: None,
            valid_time: Timestamp::UNIX_EPOCH,
            interval_seconds: None,
        })
        .unwrap();
        let field = values.get(&output).unwrap();
        assert!((field.values[0] - 301.83).abs() < 1.0e-12);
        assert_eq!(field.values[1], 0.0);
        assert_eq!(field.valid.as_ref(), &[true, false]);
    }

    #[test]
    fn geopotential_from_height_uses_declared_g0() {
        // Frozen v0 contract used by CFSR orography height (m -> m2/s2).
        let height = FieldKey::Extension(ExtensionFieldId {
            namespace: "test".into(),
            name: "height".into(),
        });
        let output = FieldKey::Canonical(CanonicalField::SurfaceGeopotential);
        let horizontal = |unit: &str| {
            ValueType::new(
                GraphUnit::parse(unit).unwrap(),
                FieldShape::Horizontal2D,
                None,
            )
            .unwrap()
        };
        let graph = ComputationGraph {
            nodes: vec![
                GraphNode {
                    id: GraphNodeId("height".into()),
                    operation: GraphOp::Source(height.clone()),
                    inputs: Vec::new(),
                    output_type: horizontal("m"),
                    stage: ExecutionStage::Frame,
                },
                GraphNode {
                    id: GraphNodeId("geopotential".into()),
                    operation: GraphOp::Thermodynamic(ThermodynamicOp::GeopotentialFromHeight),
                    inputs: vec![GraphNodeId("height".into())],
                    output_type: horizontal("m2/s2"),
                    stage: ExecutionStage::Frame,
                },
            ],
            outputs: vec![(output.clone(), GraphNodeId("geopotential".into()))],
        };
        let plan = GraphCompiler::compile(&graph).unwrap();
        let temporal = TemporalSupport::Instantaneous {
            valid_time: Timestamp::UNIX_EPOCH,
        };
        let sources = BTreeMap::from([(
            height,
            FrameComputationField::new(
                Arc::from([10.0_f64, 0.0]),
                Arc::from([true, true]),
                GraphUnit::parse("m").unwrap(),
                ArrayLayout::Horizontal2D { ny: 1, nx: 2 },
                temporal,
            )
            .unwrap(),
        )]);
        let requested = BTreeSet::from([output.clone()]);
        let values = FrameGraphExecutor::execute(FrameGraphExecutionRequest {
            graph: &graph,
            execution_plan: &plan,
            requested_outputs: &requested,
            sources: &sources,
            previous_sources: None,
            valid_time: Timestamp::UNIX_EPOCH,
            interval_seconds: None,
        })
        .unwrap();
        let field = values.get(&output).unwrap();
        assert!((field.values[0] - 98.0665).abs() < 1.0e-12);
        assert_eq!(field.values[1], 0.0);
        assert_eq!(field.valid.as_ref(), &[true, true]);
    }
}
