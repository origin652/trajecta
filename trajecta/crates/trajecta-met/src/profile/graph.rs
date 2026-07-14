//! # Contract: typed profile computation graph
//!
//! The graph is acyclic, unit-checked, shape-checked, and split into explicit
//! frame, tile, column, and sample stages. It cannot access files, networks,
//! system calls, or change horizontal topology.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde::{Deserialize, Serialize};
use trajecta_case::quantity::Dimension;

use crate::field::{FieldKey, FieldShape};
use crate::vertical::VerticalStagger;

/// Stable identifier for a graph node.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct GraphNodeId(pub String);

/// Fundamental SI dimension exponents used by the Profile type checker.
///
/// Pressure and velocity are reduced to mass, length, and time so compound
/// expressions such as `Pa / (kg/m3)` are checked physically rather than as
/// unrelated named categories.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct DimensionVector {
    /// Mass exponent.
    pub mass: i8,
    /// Length exponent.
    pub length: i8,
    /// Time exponent.
    pub time: i8,
    /// Thermodynamic-temperature exponent.
    pub temperature: i8,
}

impl DimensionVector {
    /// Dimensionless value.
    pub const DIMENSIONLESS: Self = Self::new(0, 0, 0, 0);
    /// Time.
    pub const TIME: Self = Self::new(0, 0, 1, 0);
    /// Length.
    pub const LENGTH: Self = Self::new(0, 1, 0, 0);
    /// Mass.
    pub const MASS: Self = Self::new(1, 0, 0, 0);
    /// Temperature.
    pub const TEMPERATURE: Self = Self::new(0, 0, 0, 1);
    /// Velocity.
    pub const VELOCITY: Self = Self::new(0, 1, -1, 0);
    /// Pressure.
    pub const PRESSURE: Self = Self::new(1, -1, -2, 0);

    /// Creates a dimension vector from SI base exponents.
    #[must_use]
    pub const fn new(mass: i8, length: i8, time: i8, temperature: i8) -> Self {
        Self {
            mass,
            length,
            time,
            temperature,
        }
    }

    /// Converts a Case quantity dimension into fundamental exponents.
    #[must_use]
    pub const fn from_named(dimension: Dimension) -> Self {
        match dimension {
            Dimension::Dimensionless => Self::DIMENSIONLESS,
            Dimension::Time => Self::TIME,
            Dimension::Length => Self::LENGTH,
            Dimension::Pressure => Self::PRESSURE,
            Dimension::Mass => Self::MASS,
            Dimension::Temperature => Self::TEMPERATURE,
            Dimension::Velocity => Self::VELOCITY,
        }
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

    fn checked_power(self, exponent: i8) -> Option<Self> {
        Some(Self {
            mass: self.mass.checked_mul(exponent)?,
            length: self.length.checked_mul(exponent)?,
            time: self.time.checked_mul(exponent)?,
            temperature: self.temperature.checked_mul(exponent)?,
        })
    }
}

/// Parsed, conversion-aware unit used inside Profile graphs.
#[derive(Clone, Debug, PartialEq)]
pub struct GraphUnit {
    symbol: String,
    dimension: DimensionVector,
    scale_to_si: f64,
    offset_to_si: f64,
}

impl GraphUnit {
    /// Parses a v0 unit expression such as `K`, `Pa/s`, or `kg m-2 s-1`.
    pub fn parse(symbol: &str) -> Result<Self, UnitParseError> {
        parse_unit_expression(symbol)
    }

    /// Returns the stable source spelling.
    #[must_use]
    pub fn symbol(&self) -> &str {
        &self.symbol
    }

    /// Returns fundamental physical dimensions.
    #[must_use]
    pub const fn dimension(&self) -> DimensionVector {
        self.dimension
    }

    /// Returns the multiplicative conversion into SI.
    #[must_use]
    pub const fn scale_to_si(&self) -> f64 {
        self.scale_to_si
    }

    /// Returns the additive conversion into SI.
    #[must_use]
    pub const fn offset_to_si(&self) -> f64 {
        self.offset_to_si
    }

    /// Returns whether multiplication and division are legal without first
    /// converting the value to a non-affine unit.
    #[must_use]
    pub fn is_linear(&self) -> bool {
        self.offset_to_si == 0.0
    }

    /// Converts one finite value into another compatible unit.
    pub fn convert_value_to(&self, value: f64, target: &Self) -> Result<f64, UnitParseError> {
        if !value.is_finite() {
            return Err(UnitParseError::NonFiniteValue);
        }
        if self.dimension != target.dimension {
            return Err(UnitParseError::IncompatibleDimensions {
                source: self.symbol.clone(),
                target: target.symbol.clone(),
            });
        }
        let si = value.mul_add(self.scale_to_si, self.offset_to_si);
        let converted = (si - target.offset_to_si) / target.scale_to_si;
        if !converted.is_finite() {
            return Err(UnitParseError::NonFiniteValue);
        }
        Ok(converted)
    }

    /// Returns a deterministic SI unit for a computed dimension.
    #[must_use]
    pub fn si_for_dimension(dimension: DimensionVector) -> Self {
        Self {
            symbol: canonical_si_symbol(dimension),
            dimension,
            scale_to_si: 1.0,
            offset_to_si: 0.0,
        }
    }

    /// Dimensionless SI unit.
    #[must_use]
    pub fn dimensionless() -> Self {
        Self::si_for_dimension(DimensionVector::DIMENSIONLESS)
    }

    fn same_conversion(&self, other: &Self) -> bool {
        self.dimension == other.dimension
            && self.scale_to_si == other.scale_to_si
            && self.offset_to_si == other.offset_to_si
    }
}

/// Stage at which an operation may execute.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionStage {
    /// Whole-frame or two-dimensional transformation after decoding.
    Frame,
    /// Transformation requiring a horizontal neighborhood.
    Tile,
    /// Whole-column vertical transformation.
    Column,
    /// Final query-height or surface-layer transformation.
    Sample,
}

/// Whitelisted thermodynamic operation.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ThermodynamicOp {
    /// Compute virtual temperature from temperature and specific humidity.
    VirtualTemperature,
    /// Compute density from pressure and virtual temperature.
    AirDensity,
    /// Compute relative humidity from temperature, specific humidity, and pressure.
    RelativeHumidity,
    /// Compute potential temperature from temperature and pressure.
    PotentialTemperature,
    /// Convert geopotential to geopotential height using the declared constant gravity.
    GeopotentialHeight,
    /// Convert geopotential height to geopotential using the declared constant gravity.
    GeopotentialFromHeight,
    /// Convert pressure vertical velocity to geometric vertical velocity.
    OmegaToGeometricVelocity,
    /// Convert logarithmic surface pressure into pressure.
    SurfacePressureFromLog,
}

/// Whitelisted vertical-coordinate operation.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum VerticalOp {
    /// Construct hybrid interface pressure from surface pressure and locked A/B coefficients.
    HybridInterfacePressure,
    /// Construct hybrid full-level pressure from surface pressure and locked A/B coefficients.
    HybridFullPressure,
}

/// Whitelisted computation operation.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum GraphOp {
    /// Read a mapped, unit-normalized source field.
    Source(FieldKey),
    /// Numeric dimensionless constant.
    Constant(f64),
    /// Preserve a value under a new stable node identity.
    Alias,
    /// Affine unit conversion into the declared target unit.
    ConvertUnit(GraphUnit),
    /// Arithmetic negation.
    Negate,
    /// Element-wise addition.
    Add,
    /// Element-wise subtraction.
    Subtract,
    /// Element-wise multiplication.
    Multiply,
    /// Element-wise division.
    Divide,
    /// Difference an accumulated field using its declared temporal support.
    Deaccumulate,
    /// Scalar duration of the current source interval in seconds.
    IntervalSeconds,
    /// Named, built-in thermodynamic operation.
    Thermodynamic(ThermodynamicOp),
    /// Named, built-in vertical-coordinate operation.
    Vertical(VerticalOp),
}

/// Static value type flowing through a graph edge.
#[derive(Clone, Debug, PartialEq)]
pub struct ValueType {
    /// Physical unit and conversion into SI.
    pub unit: GraphUnit,
    /// Fundamental physical dimension.
    pub dimension: DimensionVector,
    /// Array shape.
    pub shape: FieldShape,
    /// Optional vertical staggering.
    pub vertical_stagger: Option<VerticalStagger>,
}

impl ValueType {
    /// Creates a self-consistent value type.
    pub fn new(
        unit: GraphUnit,
        shape: FieldShape,
        vertical_stagger: Option<VerticalStagger>,
    ) -> Result<Self, String> {
        validate_shape_stagger(shape, vertical_stagger)?;
        Ok(Self {
            dimension: unit.dimension(),
            unit,
            shape,
            vertical_stagger,
        })
    }
}

/// One declared computation node.
#[derive(Clone, Debug, PartialEq)]
pub struct GraphNode {
    /// Stable node identifier.
    pub id: GraphNodeId,
    /// Whitelisted operation.
    pub operation: GraphOp,
    /// Ordered input nodes.
    pub inputs: Vec<GraphNodeId>,
    /// Declared output type.
    pub output_type: ValueType,
    /// Earliest legal execution stage.
    pub stage: ExecutionStage,
}

/// Uncompiled directed acyclic computation graph.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ComputationGraph {
    /// Declared nodes; identifiers must be unique.
    pub nodes: Vec<GraphNode>,
    /// Target field and producing-node pairs.
    pub outputs: Vec<(FieldKey, GraphNodeId)>,
}

/// Structural and physical graph validator.
#[derive(Clone, Copy, Debug, Default)]
pub struct GraphValidator;

impl GraphValidator {
    /// Validates cycles, identifiers, units, shapes, and legal stages.
    pub fn validate(graph: &ComputationGraph) -> Result<(), GraphError> {
        let nodes = collect_nodes(graph)?;
        validate_outputs(graph, &nodes)?;
        let order = topological_order(graph, &nodes)?;
        for id in order {
            let node = nodes
                .get(&id)
                .ok_or_else(|| GraphError::MissingNode(id.clone()))?;
            validate_node(node, &nodes)?;
        }
        Ok(())
    }
}

/// Compiler that lowers a validated graph into deterministic stages.
#[derive(Clone, Copy, Debug, Default)]
pub struct GraphCompiler;

impl GraphCompiler {
    /// Compiles a graph without executing source I/O.
    pub fn compile(graph: &ComputationGraph) -> Result<ExecutionPlan, GraphError> {
        GraphValidator::validate(graph)?;
        let nodes = collect_nodes(graph)?;
        let order = topological_order(graph, &nodes)?;
        let mut by_stage = BTreeMap::<ExecutionStage, Vec<GraphNodeId>>::new();
        for id in order {
            let stage = nodes
                .get(&id)
                .ok_or_else(|| GraphError::MissingNode(id.clone()))?
                .stage;
            by_stage.entry(stage).or_default().push(id);
        }
        Ok(ExecutionPlan {
            stages: by_stage
                .into_iter()
                .filter_map(|(stage, nodes)| {
                    (!nodes.is_empty()).then_some(CompiledStage { stage, nodes })
                })
                .collect(),
        })
    }
}

/// One compiled stage in deterministic node order.
#[derive(Clone, Debug, PartialEq)]
pub struct CompiledStage {
    /// Stage category.
    pub stage: ExecutionStage,
    /// Node identifiers in exact execution order.
    pub nodes: Vec<GraphNodeId>,
}

/// Executable, immutable graph plan.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ExecutionPlan {
    /// Ordered non-empty stages.
    pub stages: Vec<CompiledStage>,
}

fn collect_nodes(
    graph: &ComputationGraph,
) -> Result<BTreeMap<GraphNodeId, &GraphNode>, GraphError> {
    let mut nodes = BTreeMap::new();
    for node in &graph.nodes {
        if nodes.insert(node.id.clone(), node).is_some() {
            return Err(GraphError::DuplicateNode(node.id.clone()));
        }
    }
    Ok(nodes)
}

fn validate_outputs(
    graph: &ComputationGraph,
    nodes: &BTreeMap<GraphNodeId, &GraphNode>,
) -> Result<(), GraphError> {
    let mut fields = BTreeSet::new();
    for (field, id) in &graph.outputs {
        if !fields.insert(field.clone()) {
            return Err(GraphError::DuplicateOutput(field.clone()));
        }
        if !nodes.contains_key(id) {
            return Err(GraphError::MissingNode(id.clone()));
        }
    }
    Ok(())
}

fn topological_order(
    graph: &ComputationGraph,
    nodes: &BTreeMap<GraphNodeId, &GraphNode>,
) -> Result<Vec<GraphNodeId>, GraphError> {
    let mut indegree = nodes
        .keys()
        .cloned()
        .map(|id| (id, 0_usize))
        .collect::<BTreeMap<_, _>>();
    let mut dependents = BTreeMap::<GraphNodeId, BTreeSet<GraphNodeId>>::new();
    for node in &graph.nodes {
        for input in &node.inputs {
            if !nodes.contains_key(input) {
                return Err(GraphError::MissingInput {
                    node: node.id.clone(),
                    input: input.clone(),
                });
            }
            let degree = indegree
                .get_mut(&node.id)
                .ok_or_else(|| GraphError::MissingNode(node.id.clone()))?;
            *degree += 1;
            dependents
                .entry(input.clone())
                .or_default()
                .insert(node.id.clone());
        }
    }

    let mut ready = indegree
        .iter()
        .filter_map(|(id, degree)| (*degree == 0).then_some(id.clone()))
        .collect::<BTreeSet<_>>();
    let mut order = Vec::with_capacity(nodes.len());
    while let Some(id) = ready.pop_first() {
        order.push(id.clone());
        if let Some(children) = dependents.get(&id) {
            for child in children {
                let degree = indegree
                    .get_mut(child)
                    .ok_or_else(|| GraphError::MissingNode(child.clone()))?;
                *degree -= 1;
                if *degree == 0 {
                    ready.insert(child.clone());
                }
            }
        }
    }
    if order.len() != nodes.len() {
        let cycle = indegree
            .into_iter()
            .filter_map(|(id, degree)| (degree != 0).then_some(id))
            .collect();
        return Err(GraphError::Cycle(cycle));
    }
    Ok(order)
}

fn validate_node(
    node: &GraphNode,
    nodes: &BTreeMap<GraphNodeId, &GraphNode>,
) -> Result<(), GraphError> {
    if node.id.0.trim().is_empty() {
        return Err(GraphError::InvalidNodeId(node.id.clone()));
    }
    if node.output_type.dimension != node.output_type.unit.dimension() {
        return Err(type_mismatch(node, "unit and dimension vector disagree"));
    }
    validate_shape_stagger(node.output_type.shape, node.output_type.vertical_stagger).map_err(
        |message| GraphError::InvalidShape {
            node: node.id.clone(),
            message,
        },
    )?;
    if !node.output_type.unit.scale_to_si().is_finite()
        || node.output_type.unit.scale_to_si() == 0.0
        || !node.output_type.unit.offset_to_si().is_finite()
    {
        return Err(GraphError::InvalidUnit {
            node: node.id.clone(),
            message: "unit conversion must be finite with non-zero scale".into(),
        });
    }
    let inputs = node
        .inputs
        .iter()
        .map(|id| {
            nodes
                .get(id)
                .copied()
                .ok_or_else(|| GraphError::MissingInput {
                    node: node.id.clone(),
                    input: id.clone(),
                })
        })
        .collect::<Result<Vec<_>, _>>()?;

    let required_stage = inputs
        .iter()
        .map(|input| input.stage)
        .chain(std::iter::once(operation_stage(&node.operation)))
        .max()
        .unwrap_or(ExecutionStage::Frame);
    if node.stage < required_stage {
        return Err(GraphError::InvalidStage {
            node: node.id.clone(),
            required: required_stage,
            declared: node.stage,
        });
    }

    match &node.operation {
        GraphOp::Source(_) => {
            expect_arity(node, &inputs, 0)?;
            if node.stage != ExecutionStage::Frame {
                return Err(GraphError::InvalidStage {
                    node: node.id.clone(),
                    required: ExecutionStage::Frame,
                    declared: node.stage,
                });
            }
        }
        GraphOp::Constant(value) => {
            expect_arity(node, &inputs, 0)?;
            if !value.is_finite()
                || node.output_type.shape != FieldShape::Scalar
                || node.output_type.dimension != DimensionVector::DIMENSIONLESS
            {
                return Err(type_mismatch(
                    node,
                    "constants must be finite dimensionless scalars",
                ));
            }
        }
        GraphOp::Alias | GraphOp::Negate | GraphOp::Deaccumulate => {
            expect_arity(node, &inputs, 1)?;
            expect_same_type(node, &inputs[0].output_type)?;
        }
        GraphOp::ConvertUnit(target) => {
            expect_arity(node, &inputs, 1)?;
            let input = &inputs[0].output_type;
            if target.dimension() != input.dimension
                || !target.same_conversion(&node.output_type.unit)
                || input.shape != node.output_type.shape
                || input.vertical_stagger != node.output_type.vertical_stagger
            {
                return Err(type_mismatch(
                    node,
                    "unit conversion changed dimension, shape, or staggering",
                ));
            }
        }
        GraphOp::Add | GraphOp::Subtract => {
            expect_arity(node, &inputs, 2)?;
            let left = &inputs[0].output_type;
            let right = &inputs[1].output_type;
            let (shape, stagger) = broadcast_shape(left, right).ok_or_else(|| {
                type_mismatch(node, "addition input shapes or staggering are incompatible")
            })?;
            if left.dimension != right.dimension
                || !left.unit.same_conversion(&right.unit)
                || node.output_type.dimension != left.dimension
                || !node.output_type.unit.same_conversion(&left.unit)
                || node.output_type.shape != shape
                || node.output_type.vertical_stagger != stagger
            {
                return Err(type_mismatch(
                    node,
                    "addition and subtraction require equal dimensions and converted units",
                ));
            }
        }
        GraphOp::Multiply | GraphOp::Divide => {
            expect_arity(node, &inputs, 2)?;
            validate_product(node, &inputs, matches!(node.operation, GraphOp::Divide))?;
        }
        GraphOp::IntervalSeconds => {
            expect_arity(node, &inputs, 0)?;
            let seconds = GraphUnit::parse("s").map_err(|error| GraphError::InvalidUnit {
                node: node.id.clone(),
                message: error.to_string(),
            })?;
            let expected =
                ValueType::new(seconds, FieldShape::Scalar, None).map_err(|message| {
                    GraphError::InvalidShape {
                        node: node.id.clone(),
                        message,
                    }
                })?;
            expect_same_type(node, &expected)?;
        }
        GraphOp::Thermodynamic(operation) => {
            validate_thermodynamic(node, &inputs, *operation)?;
        }
        GraphOp::Vertical(operation) => validate_vertical(node, &inputs, *operation)?,
    }
    Ok(())
}

fn operation_stage(operation: &GraphOp) -> ExecutionStage {
    match operation {
        GraphOp::Vertical(_) => ExecutionStage::Column,
        _ => ExecutionStage::Frame,
    }
}

fn expect_arity(
    node: &GraphNode,
    inputs: &[&GraphNode],
    expected: usize,
) -> Result<(), GraphError> {
    if inputs.len() != expected {
        return Err(GraphError::WrongArity {
            node: node.id.clone(),
            expected,
            actual: inputs.len(),
        });
    }
    Ok(())
}

fn expect_same_type(node: &GraphNode, expected: &ValueType) -> Result<(), GraphError> {
    if &node.output_type != expected {
        return Err(type_mismatch(
            node,
            "declared output type does not match input",
        ));
    }
    Ok(())
}

fn validate_product(
    node: &GraphNode,
    inputs: &[&GraphNode],
    divide: bool,
) -> Result<(), GraphError> {
    let left = &inputs[0].output_type;
    let right = &inputs[1].output_type;
    if !left.unit.is_linear() || !right.unit.is_linear() {
        return Err(type_mismatch(
            node,
            "affine units must be converted before multiplication or division",
        ));
    }
    let dimension = if divide {
        left.dimension.checked_quotient(right.dimension)
    } else {
        left.dimension.checked_product(right.dimension)
    }
    .ok_or_else(|| type_mismatch(node, "dimension exponent overflow"))?;
    let (shape, stagger) = broadcast_shape(left, right).ok_or_else(|| {
        type_mismatch(node, "array shapes or vertical staggering are incompatible")
    })?;
    if node.output_type.dimension != dimension
        || node.output_type.shape != shape
        || node.output_type.vertical_stagger != stagger
        || !node.output_type.unit.is_linear()
    {
        return Err(type_mismatch(node, "product output type is inconsistent"));
    }
    Ok(())
}

fn validate_thermodynamic(
    node: &GraphNode,
    inputs: &[&GraphNode],
    operation: ThermodynamicOp,
) -> Result<(), GraphError> {
    let temperature = DimensionVector::TEMPERATURE;
    let pressure = DimensionVector::PRESSURE;
    let dimensionless = DimensionVector::DIMENSIONLESS;
    let density = DimensionVector::new(1, -3, 0, 0);
    let geopotential = DimensionVector::new(0, 2, -2, 0);
    let pressure_rate = DimensionVector::new(1, -1, -3, 0);
    let (expected_dimensions, output_dimension) = match operation {
        ThermodynamicOp::VirtualTemperature => (vec![temperature, dimensionless], temperature),
        ThermodynamicOp::AirDensity => (vec![pressure, temperature], density),
        ThermodynamicOp::RelativeHumidity => {
            (vec![temperature, dimensionless, pressure], dimensionless)
        }
        ThermodynamicOp::PotentialTemperature => (vec![temperature, pressure], temperature),
        ThermodynamicOp::GeopotentialHeight => (vec![geopotential], DimensionVector::LENGTH),
        ThermodynamicOp::GeopotentialFromHeight => (vec![DimensionVector::LENGTH], geopotential),
        ThermodynamicOp::OmegaToGeometricVelocity => {
            (vec![pressure_rate, density], DimensionVector::VELOCITY)
        }
        ThermodynamicOp::SurfacePressureFromLog => (vec![dimensionless], pressure),
    };
    expect_arity(node, inputs, expected_dimensions.len())?;
    for (input, expected) in inputs.iter().zip(expected_dimensions) {
        if input.output_type.dimension != expected {
            return Err(type_mismatch(
                node,
                "thermodynamic input dimension mismatch",
            ));
        }
    }
    let (shape, stagger) = broadcast_many(inputs)
        .ok_or_else(|| type_mismatch(node, "thermodynamic input shapes are incompatible"))?;
    if node.output_type.dimension != output_dimension
        || node.output_type.shape != shape
        || node.output_type.vertical_stagger != stagger
    {
        return Err(type_mismatch(
            node,
            "thermodynamic output type is inconsistent",
        ));
    }
    Ok(())
}

fn validate_vertical(
    node: &GraphNode,
    inputs: &[&GraphNode],
    operation: VerticalOp,
) -> Result<(), GraphError> {
    expect_arity(node, inputs, 1)?;
    let input = &inputs[0].output_type;
    if input.dimension != DimensionVector::PRESSURE || input.shape != FieldShape::Horizontal2D {
        return Err(type_mismatch(
            node,
            "hybrid pressure construction requires two-dimensional surface pressure",
        ));
    }
    let (shape, stagger) = match operation {
        VerticalOp::HybridInterfacePressure => {
            (FieldShape::Interface3D, Some(VerticalStagger::Interface))
        }
        VerticalOp::HybridFullPressure => (FieldShape::Full3D, Some(VerticalStagger::Full)),
    };
    if node.output_type.dimension != DimensionVector::PRESSURE
        || node.output_type.shape != shape
        || node.output_type.vertical_stagger != stagger
    {
        return Err(type_mismatch(
            node,
            "hybrid pressure output type is inconsistent",
        ));
    }
    Ok(())
}

fn broadcast_many(inputs: &[&GraphNode]) -> Option<(FieldShape, Option<VerticalStagger>)> {
    let first = inputs.first()?.output_type.clone();
    inputs.iter().skip(1).try_fold(
        (first.shape, first.vertical_stagger),
        |(shape, stagger), input| {
            let left = ValueType {
                unit: GraphUnit::dimensionless(),
                dimension: DimensionVector::DIMENSIONLESS,
                shape,
                vertical_stagger: stagger,
            };
            broadcast_shape(&left, &input.output_type)
        },
    )
}

fn broadcast_shape(
    left: &ValueType,
    right: &ValueType,
) -> Option<(FieldShape, Option<VerticalStagger>)> {
    match (left.shape, right.shape) {
        (FieldShape::Scalar, _) => Some((right.shape, right.vertical_stagger)),
        (_, FieldShape::Scalar) => Some((left.shape, left.vertical_stagger)),
        (left_shape, right_shape)
            if left_shape == right_shape && left.vertical_stagger == right.vertical_stagger =>
        {
            Some((left_shape, left.vertical_stagger))
        }
        _ => None,
    }
}

fn validate_shape_stagger(
    shape: FieldShape,
    stagger: Option<VerticalStagger>,
) -> Result<(), String> {
    match (shape, stagger) {
        (FieldShape::Scalar | FieldShape::Horizontal2D, None)
        | (FieldShape::Full3D, Some(VerticalStagger::Full))
        | (FieldShape::Interface3D, Some(VerticalStagger::Interface)) => Ok(()),
        _ => Err("shape and vertical staggering disagree".into()),
    }
}

fn type_mismatch(node: &GraphNode, message: &str) -> GraphError {
    GraphError::TypeMismatch {
        node: node.id.clone(),
        message: message.into(),
    }
}

fn parse_unit_expression(input: &str) -> Result<GraphUnit, UnitParseError> {
    let symbol = input.trim();
    if symbol.is_empty() {
        return Err(UnitParseError::Empty);
    }
    if symbol == "1" {
        return Ok(GraphUnit::dimensionless());
    }

    let normalized = symbol.replace(['*', '_'], " ");
    let mut dimension = DimensionVector::DIMENSIONLESS;
    let mut scale_to_si = 1.0;
    let mut saw_factor = false;
    let mut denominator = false;
    let mut affine = None;
    for slash_part in normalized.split('/') {
        for raw_factor in slash_part.split_whitespace() {
            if affine.is_some() {
                return Err(UnitParseError::AffineCompound(symbol.into()));
            }
            let (base, exponent) = parse_unit_factor(raw_factor)?;
            let exponent = if denominator { -exponent } else { exponent };
            let definition =
                base_unit(base).ok_or_else(|| UnitParseError::UnknownUnit(base.into()))?;
            if definition.offset_to_si != 0.0 {
                if affine.is_some() || saw_factor || exponent != 1 {
                    return Err(UnitParseError::AffineCompound(symbol.into()));
                }
                affine = Some(definition.offset_to_si);
            }
            dimension = dimension
                .checked_product(
                    definition
                        .dimension
                        .checked_power(exponent)
                        .ok_or(UnitParseError::ExponentOverflow)?,
                )
                .ok_or(UnitParseError::ExponentOverflow)?;
            scale_to_si *= definition.scale_to_si.powi(i32::from(exponent));
            saw_factor = true;
        }
        denominator = true;
    }
    if !saw_factor || !scale_to_si.is_finite() || scale_to_si == 0.0 {
        return Err(UnitParseError::Invalid(input.into()));
    }
    Ok(GraphUnit {
        symbol: symbol.into(),
        dimension,
        scale_to_si,
        offset_to_si: affine.unwrap_or(0.0),
    })
}

fn parse_unit_factor(factor: &str) -> Result<(&str, i8), UnitParseError> {
    if factor.is_empty() {
        return Err(UnitParseError::Invalid(factor.into()));
    }
    if let Some((base, exponent)) = factor.split_once('^') {
        let exponent = exponent
            .parse::<i8>()
            .map_err(|_| UnitParseError::InvalidExponent(factor.into()))?;
        if base.is_empty() {
            return Err(UnitParseError::Invalid(factor.into()));
        }
        return Ok((base, exponent));
    }
    let split = factor
        .char_indices()
        .find_map(|(index, character)| {
            (index > 0 && (character == '-' || character.is_ascii_digit())).then_some(index)
        })
        .unwrap_or(factor.len());
    let (base, raw_exponent) = factor.split_at(split);
    let exponent = if raw_exponent.is_empty() {
        1
    } else {
        raw_exponent
            .parse::<i8>()
            .map_err(|_| UnitParseError::InvalidExponent(factor.into()))?
    };
    if base.is_empty() {
        return Err(UnitParseError::Invalid(factor.into()));
    }
    Ok((base, exponent))
}

#[derive(Clone, Copy)]
struct BaseUnit {
    dimension: DimensionVector,
    scale_to_si: f64,
    offset_to_si: f64,
}

fn base_unit(symbol: &str) -> Option<BaseUnit> {
    let unit = match symbol {
        "s" => BaseUnit {
            dimension: DimensionVector::TIME,
            scale_to_si: 1.0,
            offset_to_si: 0.0,
        },
        "min" => BaseUnit {
            dimension: DimensionVector::TIME,
            scale_to_si: 60.0,
            offset_to_si: 0.0,
        },
        "h" => BaseUnit {
            dimension: DimensionVector::TIME,
            scale_to_si: 3_600.0,
            offset_to_si: 0.0,
        },
        "day" => BaseUnit {
            dimension: DimensionVector::TIME,
            scale_to_si: 86_400.0,
            offset_to_si: 0.0,
        },
        "m" => BaseUnit {
            dimension: DimensionVector::LENGTH,
            scale_to_si: 1.0,
            offset_to_si: 0.0,
        },
        "km" => BaseUnit {
            dimension: DimensionVector::LENGTH,
            scale_to_si: 1_000.0,
            offset_to_si: 0.0,
        },
        "cm" => BaseUnit {
            dimension: DimensionVector::LENGTH,
            scale_to_si: 0.01,
            offset_to_si: 0.0,
        },
        "kg" => BaseUnit {
            dimension: DimensionVector::MASS,
            scale_to_si: 1.0,
            offset_to_si: 0.0,
        },
        "g" => BaseUnit {
            dimension: DimensionVector::MASS,
            scale_to_si: 0.001,
            offset_to_si: 0.0,
        },
        "K" => BaseUnit {
            dimension: DimensionVector::TEMPERATURE,
            scale_to_si: 1.0,
            offset_to_si: 0.0,
        },
        "degC" => BaseUnit {
            dimension: DimensionVector::TEMPERATURE,
            scale_to_si: 1.0,
            offset_to_si: 273.15,
        },
        "Pa" => BaseUnit {
            dimension: DimensionVector::PRESSURE,
            scale_to_si: 1.0,
            offset_to_si: 0.0,
        },
        "hPa" | "mbar" => BaseUnit {
            dimension: DimensionVector::PRESSURE,
            scale_to_si: 100.0,
            offset_to_si: 0.0,
        },
        _ => return None,
    };
    Some(unit)
}

fn canonical_si_symbol(dimension: DimensionVector) -> String {
    if dimension == DimensionVector::DIMENSIONLESS {
        return "1".into();
    }
    let factors = [
        ("kg", dimension.mass),
        ("m", dimension.length),
        ("s", dimension.time),
        ("K", dimension.temperature),
    ];
    factors
        .into_iter()
        .filter(|(_, exponent)| *exponent != 0)
        .map(|(base, exponent)| {
            if exponent == 1 {
                base.into()
            } else {
                format!("{base}{exponent}")
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Unit-expression parsing failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum UnitParseError {
    /// Unit text was empty.
    Empty,
    /// A base symbol is outside the v0 unit whitelist.
    UnknownUnit(String),
    /// A factor exponent is malformed.
    InvalidExponent(String),
    /// Dimension exponent arithmetic overflowed.
    ExponentOverflow,
    /// An affine unit was used inside a product or quotient.
    AffineCompound(String),
    /// Unit syntax is otherwise invalid.
    Invalid(String),
    /// Numeric conversion received or produced NaN or infinity.
    NonFiniteValue,
    /// Conversion was requested between different physical dimensions.
    IncompatibleDimensions {
        /// Source unit symbol.
        source: String,
        /// Target unit symbol.
        target: String,
    },
}

impl fmt::Display for UnitParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => formatter.write_str("unit expression is empty"),
            Self::UnknownUnit(symbol) => write!(formatter, "unknown unit symbol '{symbol}'"),
            Self::InvalidExponent(factor) => {
                write!(formatter, "invalid unit exponent in '{factor}'")
            }
            Self::ExponentOverflow => formatter.write_str("unit dimension exponent overflow"),
            Self::AffineCompound(unit) => {
                write!(formatter, "affine unit '{unit}' cannot be compounded")
            }
            Self::Invalid(unit) => write!(formatter, "invalid unit expression '{unit}'"),
            Self::NonFiniteValue => formatter.write_str("unit conversion value must be finite"),
            Self::IncompatibleDimensions { source, target } => write!(
                formatter,
                "cannot convert incompatible units '{source}' and '{target}'"
            ),
        }
    }
}

impl std::error::Error for UnitParseError {}

/// Graph validation or compilation failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum GraphError {
    /// A node identifier is empty.
    InvalidNodeId(GraphNodeId),
    /// Node identifiers are not unique.
    DuplicateNode(GraphNodeId),
    /// An output field has more than one graph producer.
    DuplicateOutput(FieldKey),
    /// A referenced output node does not exist.
    MissingNode(GraphNodeId),
    /// One node references an absent input.
    MissingInput {
        /// Referencing node.
        node: GraphNodeId,
        /// Missing input.
        input: GraphNodeId,
    },
    /// The graph contains a cycle.
    Cycle(Vec<GraphNodeId>),
    /// An operation receives an incompatible value type.
    TypeMismatch {
        /// Failing node.
        node: GraphNodeId,
        /// Stable explanation.
        message: String,
    },
    /// Unit metadata is invalid.
    InvalidUnit {
        /// Failing node.
        node: GraphNodeId,
        /// Stable explanation.
        message: String,
    },
    /// Shape and staggering metadata disagree.
    InvalidShape {
        /// Failing node.
        node: GraphNodeId,
        /// Stable explanation.
        message: String,
    },
    /// An operation has the wrong number of inputs.
    WrongArity {
        /// Failing node.
        node: GraphNodeId,
        /// Required input count.
        expected: usize,
        /// Actual input count.
        actual: usize,
    },
    /// An operation is assigned to an illegal execution stage.
    InvalidStage {
        /// Failing node.
        node: GraphNodeId,
        /// Earliest legal stage.
        required: ExecutionStage,
        /// Declared stage.
        declared: ExecutionStage,
    },
}

impl fmt::Display for GraphError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidNodeId(id) => write!(formatter, "invalid graph node id '{}'", id.0),
            Self::DuplicateNode(id) => write!(formatter, "duplicate graph node '{}'", id.0),
            Self::DuplicateOutput(field) => write!(formatter, "duplicate graph output {field:?}"),
            Self::MissingNode(id) => write!(formatter, "missing graph node '{}'", id.0),
            Self::MissingInput { node, input } => {
                write!(
                    formatter,
                    "graph node '{}' references missing input '{}'",
                    node.0, input.0
                )
            }
            Self::Cycle(ids) => write!(
                formatter,
                "graph cycle contains {}",
                ids.iter()
                    .map(|id| id.0.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Self::TypeMismatch { node, message } => {
                write!(
                    formatter,
                    "graph node '{}' type mismatch: {message}",
                    node.0
                )
            }
            Self::InvalidUnit { node, message } => {
                write!(
                    formatter,
                    "graph node '{}' has invalid unit: {message}",
                    node.0
                )
            }
            Self::InvalidShape { node, message } => {
                write!(
                    formatter,
                    "graph node '{}' has invalid shape: {message}",
                    node.0
                )
            }
            Self::WrongArity {
                node,
                expected,
                actual,
            } => write!(
                formatter,
                "graph node '{}' expects {expected} inputs but has {actual}",
                node.0
            ),
            Self::InvalidStage {
                node,
                required,
                declared,
            } => write!(
                formatter,
                "graph node '{}' requires stage {required:?} but declares {declared:?}",
                node.0
            ),
        }
    }
}

impl std::error::Error for GraphError {}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use crate::field::CanonicalField;

    use super::*;

    fn scalar(unit: &str) -> ValueType {
        ValueType::new(GraphUnit::parse(unit).unwrap(), FieldShape::Scalar, None).unwrap()
    }

    fn node(id: &str, operation: GraphOp, inputs: &[&str], output_type: ValueType) -> GraphNode {
        GraphNode {
            id: GraphNodeId(id.into()),
            operation,
            inputs: inputs.iter().map(|id| GraphNodeId((*id).into())).collect(),
            output_type,
            stage: ExecutionStage::Frame,
        }
    }

    #[test]
    fn compound_units_reduce_to_fundamental_dimensions() {
        let pressure_rate = GraphUnit::parse("Pa/s").unwrap();
        assert_eq!(
            pressure_rate.dimension(),
            DimensionVector::new(1, -1, -3, 0)
        );
        let density = GraphUnit::parse("kg m-3").unwrap();
        assert_eq!(density.dimension(), DimensionVector::new(1, -3, 0, 0));
        assert_eq!(
            GraphUnit::parse("m/s").unwrap().dimension(),
            DimensionVector::VELOCITY
        );
        assert!(GraphUnit::parse("degC/s").is_err());
    }

    #[test]
    fn compiler_is_topological_and_deterministic() {
        let graph = ComputationGraph {
            nodes: vec![
                node("sum", GraphOp::Add, &["left", "right"], scalar("K")),
                node("right", GraphOp::Constant(2.0), &[], scalar("1")),
                node("left", GraphOp::Constant(1.0), &[], scalar("1")),
            ],
            outputs: vec![(
                FieldKey::Canonical(CanonicalField::AirTemperature),
                GraphNodeId("sum".into()),
            )],
        };
        assert!(matches!(
            GraphCompiler::compile(&graph),
            Err(GraphError::TypeMismatch { .. })
        ));

        let mut graph = graph;
        graph.nodes[0].output_type = scalar("1");
        let plan = GraphCompiler::compile(&graph).unwrap();
        assert_eq!(
            plan.stages[0].nodes,
            vec![
                GraphNodeId("left".into()),
                GraphNodeId("right".into()),
                GraphNodeId("sum".into())
            ]
        );
    }

    #[test]
    fn cycles_and_early_stages_are_rejected() {
        let cyclic = ComputationGraph {
            nodes: vec![
                node("a", GraphOp::Alias, &["b"], scalar("1")),
                node("b", GraphOp::Alias, &["a"], scalar("1")),
            ],
            outputs: Vec::new(),
        };
        assert!(matches!(
            GraphValidator::validate(&cyclic),
            Err(GraphError::Cycle(_))
        ));

        let mut vertical = node(
            "pressure",
            GraphOp::Vertical(VerticalOp::HybridFullPressure),
            &["surface"],
            ValueType::new(
                GraphUnit::parse("Pa").unwrap(),
                FieldShape::Full3D,
                Some(VerticalStagger::Full),
            )
            .unwrap(),
        );
        let surface = node(
            "surface",
            GraphOp::Source(FieldKey::Canonical(CanonicalField::SurfacePressure)),
            &[],
            ValueType::new(
                GraphUnit::parse("Pa").unwrap(),
                FieldShape::Horizontal2D,
                None,
            )
            .unwrap(),
        );
        vertical.stage = ExecutionStage::Frame;
        let graph = ComputationGraph {
            nodes: vec![surface, vertical],
            outputs: Vec::new(),
        };
        assert!(matches!(
            GraphValidator::validate(&graph),
            Err(GraphError::InvalidStage { .. })
        ));
    }
}
