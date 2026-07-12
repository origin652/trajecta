//! # Contract: typed profile computation graph
//!
//! The graph is acyclic, unit-checked, shape-checked, and split into explicit
//! frame, tile, column, and sample stages. It cannot access files, networks,
//! system calls, or change horizontal topology.

use trajecta_case::quantity::{Dimension, Unit};

use crate::field::{FieldKey, FieldShape};
use crate::vertical::VerticalStagger;

/// Stable identifier for a graph node.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct GraphNodeId(pub String);

/// Stage at which an operation may execute.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
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

/// Whitelisted computation operation.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum GraphOp {
    /// Read a mapped source field.
    Source(FieldKey),
    /// Numeric constant with declared unit.
    Constant(f64),
    /// Affine unit conversion.
    ConvertUnit,
    /// Element-wise addition.
    Add,
    /// Element-wise subtraction.
    Subtract,
    /// Element-wise multiplication.
    Multiply,
    /// Element-wise division.
    Divide,
    /// Named, built-in thermodynamic operation.
    Thermodynamic(String),
    /// Named, built-in vertical-coordinate operation.
    Vertical(String),
}

/// Static value type flowing through a graph edge.
#[derive(Clone, Debug, PartialEq)]
pub struct ValueType {
    /// Physical unit.
    pub unit: Unit,
    /// Physical dimension.
    pub dimension: Dimension,
    /// Array shape.
    pub shape: FieldShape,
    /// Optional vertical staggering.
    pub vertical_stagger: Option<VerticalStagger>,
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
    pub fn validate(_graph: &ComputationGraph) -> Result<(), GraphError> {
        Err(GraphError::NotImplemented)
    }
}

/// Compiler that lowers a validated graph into deterministic stages.
#[derive(Clone, Copy, Debug, Default)]
pub struct GraphCompiler;

impl GraphCompiler {
    /// Compiles a graph without executing source I/O.
    pub fn compile(_graph: &ComputationGraph) -> Result<ExecutionPlan, GraphError> {
        Err(GraphError::NotImplemented)
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

/// Graph validation or compilation failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum GraphError {
    /// Graph algorithms have not been implemented yet.
    NotImplemented,
    /// Node identifiers are not unique.
    DuplicateNode(GraphNodeId),
    /// A referenced node does not exist.
    MissingNode(GraphNodeId),
    /// The graph contains a cycle.
    Cycle(Vec<GraphNodeId>),
    /// An operation receives an incompatible value type.
    TypeMismatch(GraphNodeId),
    /// An operation is assigned to an illegal execution stage.
    InvalidStage(GraphNodeId),
}
