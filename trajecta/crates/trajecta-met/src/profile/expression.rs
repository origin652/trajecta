//! # Contract: bounded Profile expression language
//!
//! The v0 language supports identifiers, finite numeric constants, arithmetic,
//! parentheses, and a fixed set of built-in scientific functions. Parsing and
//! compilation are bounded and deterministic. There are no strings, loops,
//! user-defined functions, recursion, external calls, or implicit field lookup.
//! Canonical fields use built-in types; direct namespaced extension fields must
//! have an explicit unit and shape descriptor in the Profile document.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use crate::field::{CanonicalField, FieldKey, FieldShape};
use crate::profile::document::{DatasetProfileDocument, ExtensionFieldDescriptor, FieldReference};
use crate::profile::graph::{
    ComputationGraph, DimensionVector, ExecutionPlan, ExecutionStage, GraphCompiler, GraphError,
    GraphNode, GraphNodeId, GraphOp, GraphUnit, ThermodynamicOp, UnitParseError, ValueType,
    VerticalOp,
};
use crate::vertical::VerticalStagger;

const MAX_TOKENS: usize = 4_096;
const MAX_NESTING: usize = 128;

/// Parsed expression tree.
#[derive(Clone, Debug, PartialEq)]
pub enum Expression {
    /// Exact symbolic field reference.
    Identifier(String),
    /// Finite dimensionless numeric constant.
    Constant(f64),
    /// Unary arithmetic operation.
    Unary {
        /// Operator.
        operator: UnaryOperator,
        /// Operand.
        operand: Box<Self>,
    },
    /// Binary arithmetic operation.
    Binary {
        /// Operator.
        operator: BinaryOperator,
        /// Left operand.
        left: Box<Self>,
        /// Right operand.
        right: Box<Self>,
    },
    /// Whitelisted function call; function validity is checked during compilation.
    Call {
        /// Exact function name.
        function: String,
        /// Ordered arguments.
        arguments: Vec<Self>,
    },
}

/// Unary arithmetic operator.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UnaryOperator {
    /// Numeric negation.
    Negate,
}

/// Binary arithmetic operator.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BinaryOperator {
    /// Addition.
    Add,
    /// Subtraction.
    Subtract,
    /// Multiplication.
    Multiply,
    /// Division.
    Divide,
}

/// A validated graph and deterministic execution order compiled from a Profile.
#[derive(Clone, Debug, PartialEq)]
pub struct CompiledProfileGraph {
    /// Typed graph containing direct and derived outputs.
    pub graph: ComputationGraph,
    /// Deterministic topological stage plan.
    pub execution_plan: ExecutionPlan,
}

/// Parses one bounded Profile expression.
pub fn parse_expression(input: &str) -> Result<Expression, ExpressionError> {
    let tokens = Lexer::new(input).tokenize()?;
    Parser::new(tokens).parse()
}

/// Parses, type-checks, and compiles every direct and derived Profile field.
pub fn compile_profile_graph(
    document: &DatasetProfileDocument,
) -> Result<CompiledProfileGraph, ExpressionError> {
    let extension_types = extension_value_types(&document.extension_fields)?;
    let mut parsed = BTreeMap::new();
    for derived in &document.derived_fields {
        let expression = parse_expression(&derived.expression).map_err(|error| {
            ExpressionError::InDerivedField {
                field: derived.id.clone(),
                message: error.to_string(),
            }
        })?;
        parsed.insert(derived.id.clone(), expression);
    }

    let direct_ids = document
        .fields
        .iter()
        .map(|mapping| mapping.id.clone())
        .collect::<BTreeSet<_>>();
    let derived_ids = document
        .derived_fields
        .iter()
        .map(|derived| derived.id.clone())
        .collect::<BTreeSet<_>>();
    let derived_order = derived_topological_order(&parsed, &direct_ids, &derived_ids)?;

    let mut builder = GraphBuilder::default();
    for mapping in &document.fields {
        let field_key = mapping.target.to_field_key();
        let output_type = registered_value_type(&mapping.target, &extension_types)?;
        for source in &mapping.sources {
            let source_unit = GraphUnit::parse(&source.unit).map_err(|error| {
                ExpressionError::InvalidSourceUnit {
                    field: mapping.id.clone(),
                    unit: source.unit.clone(),
                    message: error.to_string(),
                }
            })?;
            if source_unit.dimension() != output_type.dimension {
                return Err(ExpressionError::InvalidSourceUnit {
                    field: mapping.id.clone(),
                    unit: source.unit.clone(),
                    message: format!(
                        "dimension {:?} is incompatible with canonical dimension {:?}",
                        source_unit.dimension(),
                        output_type.dimension
                    ),
                });
            }
        }
        let id = GraphNodeId(format!("source:{}", mapping.id));
        builder.nodes.push(GraphNode {
            id: id.clone(),
            operation: GraphOp::Source(field_key.clone()),
            inputs: Vec::new(),
            output_type: output_type.clone(),
            stage: ExecutionStage::Frame,
        });
        builder.bindings.insert(
            mapping.id.clone(),
            Binding {
                node: id.clone(),
                value_type: output_type,
                stage: ExecutionStage::Frame,
            },
        );
        builder.outputs.push((field_key, id));
    }

    let derived_by_id = document
        .derived_fields
        .iter()
        .map(|field| (field.id.as_str(), field))
        .collect::<BTreeMap<_, _>>();
    for id in derived_order {
        let field = derived_by_id
            .get(id.as_str())
            .ok_or_else(|| ExpressionError::UnknownIdentifier(id.clone()))?;
        let expression = parsed
            .get(&id)
            .ok_or_else(|| ExpressionError::UnknownIdentifier(id.clone()))?;
        let inferred = builder.compile(expression)?;
        if field.stage < inferred.stage {
            return Err(ExpressionError::StageTooEarly {
                field: field.id.clone(),
                required: inferred.stage,
                declared: field.stage,
            });
        }

        let declared_unit = GraphUnit::parse(&field.unit).map_err(|error| {
            ExpressionError::InvalidDeclaredUnit {
                field: field.id.clone(),
                unit: field.unit.clone(),
                message: error.to_string(),
            }
        })?;
        let declared_stagger = stagger_for_shape(field.shape);
        let declared_type =
            ValueType::new(declared_unit, field.shape, declared_stagger).map_err(|message| {
                ExpressionError::InvalidDeclaredType {
                    field: field.id.clone(),
                    message,
                }
            })?;
        if inferred.value_type.dimension != declared_type.dimension
            || inferred.value_type.shape != declared_type.shape
            || inferred.value_type.vertical_stagger != declared_type.vertical_stagger
        {
            return Err(ExpressionError::DeclaredTypeMismatch {
                field: field.id.clone(),
                message: format!(
                    "expression has dimension {:?}, shape {:?}, stagger {:?}; declaration has dimension {:?}, shape {:?}, stagger {:?}",
                    inferred.value_type.dimension,
                    inferred.value_type.shape,
                    inferred.value_type.vertical_stagger,
                    declared_type.dimension,
                    declared_type.shape,
                    declared_type.vertical_stagger
                ),
            });
        }
        let declared = builder.convert(inferred, &declared_type.unit)?;
        let target_type = match &field.target {
            FieldReference::Canonical(_) => registered_value_type(&field.target, &extension_types)?,
            FieldReference::Extension { .. } => extension_types
                .get(&field.target)
                .cloned()
                .unwrap_or_else(|| declared_type.clone()),
        };
        if declared.value_type.dimension != target_type.dimension
            || declared.value_type.shape != target_type.shape
            || declared.value_type.vertical_stagger != target_type.vertical_stagger
        {
            return Err(ExpressionError::CanonicalTypeMismatch {
                field: field.id.clone(),
                target: field.target.clone(),
            });
        }
        let canonical = builder.convert(declared, &target_type.unit)?;
        let root = GraphNodeId(format!("derived:{}", field.id));
        builder.nodes.push(GraphNode {
            id: root.clone(),
            operation: GraphOp::Alias,
            inputs: vec![canonical.node],
            output_type: target_type.clone(),
            stage: field.stage,
        });
        let field_key = field.target.to_field_key();
        builder.outputs.push((field_key, root.clone()));
        builder.bindings.insert(
            field.id.clone(),
            Binding {
                node: root,
                value_type: target_type,
                stage: field.stage,
            },
        );
    }

    let graph = ComputationGraph {
        nodes: builder.nodes,
        outputs: builder.outputs,
    };
    let execution_plan = GraphCompiler::compile(&graph).map_err(ExpressionError::Graph)?;
    Ok(CompiledProfileGraph {
        graph,
        execution_plan,
    })
}

fn derived_topological_order(
    expressions: &BTreeMap<String, Expression>,
    direct_ids: &BTreeSet<String>,
    derived_ids: &BTreeSet<String>,
) -> Result<Vec<String>, ExpressionError> {
    let mut dependencies = BTreeMap::<String, BTreeSet<String>>::new();
    let mut dependents = BTreeMap::<String, BTreeSet<String>>::new();
    for (id, expression) in expressions {
        let mut identifiers = BTreeSet::new();
        collect_identifiers(expression, &mut identifiers);
        let mut derived_dependencies = BTreeSet::new();
        for identifier in identifiers {
            if direct_ids.contains(&identifier) {
                continue;
            }
            if !derived_ids.contains(&identifier) {
                return Err(ExpressionError::UnknownIdentifier(identifier));
            }
            derived_dependencies.insert(identifier.clone());
            dependents.entry(identifier).or_default().insert(id.clone());
        }
        dependencies.insert(id.clone(), derived_dependencies);
    }
    let mut ready = dependencies
        .iter()
        .filter_map(|(id, values)| values.is_empty().then_some(id.clone()))
        .collect::<BTreeSet<_>>();
    let mut order = Vec::with_capacity(dependencies.len());
    while let Some(id) = ready.pop_first() {
        order.push(id.clone());
        if let Some(children) = dependents.get(&id) {
            for child in children {
                let child_dependencies = dependencies
                    .get_mut(child)
                    .ok_or_else(|| ExpressionError::UnknownIdentifier(child.clone()))?;
                child_dependencies.remove(&id);
                if child_dependencies.is_empty() {
                    ready.insert(child.clone());
                }
            }
        }
    }
    if order.len() != dependencies.len() {
        return Err(ExpressionError::DependencyCycle(
            dependencies
                .into_iter()
                .filter_map(|(id, values)| (!values.is_empty()).then_some(id))
                .collect(),
        ));
    }
    Ok(order)
}

fn collect_identifiers(expression: &Expression, identifiers: &mut BTreeSet<String>) {
    match expression {
        Expression::Identifier(identifier) => {
            identifiers.insert(identifier.clone());
        }
        Expression::Constant(_) => {}
        Expression::Unary { operand, .. } => collect_identifiers(operand, identifiers),
        Expression::Binary { left, right, .. } => {
            collect_identifiers(left, identifiers);
            collect_identifiers(right, identifiers);
        }
        Expression::Call { arguments, .. } => {
            for argument in arguments {
                collect_identifiers(argument, identifiers);
            }
        }
    }
}

#[derive(Clone)]
struct Binding {
    node: GraphNodeId,
    value_type: ValueType,
    stage: ExecutionStage,
}

#[derive(Default)]
struct GraphBuilder {
    nodes: Vec<GraphNode>,
    outputs: Vec<(FieldKey, GraphNodeId)>,
    bindings: BTreeMap<String, Binding>,
    next_node: u64,
}

impl GraphBuilder {
    fn compile(&mut self, expression: &Expression) -> Result<Binding, ExpressionError> {
        match expression {
            Expression::Identifier(identifier) => self
                .bindings
                .get(identifier)
                .cloned()
                .ok_or_else(|| ExpressionError::UnknownIdentifier(identifier.clone())),
            Expression::Constant(value) => {
                let value_type =
                    ValueType::new(GraphUnit::dimensionless(), FieldShape::Scalar, None)
                        .map_err(|message| ExpressionError::Internal(message.clone()))?;
                self.push(
                    GraphOp::Constant(*value),
                    Vec::new(),
                    value_type,
                    ExecutionStage::Frame,
                )
            }
            Expression::Unary { operator, operand } => {
                let operand = self.compile(operand)?;
                match operator {
                    UnaryOperator::Negate => self.push(
                        GraphOp::Negate,
                        vec![operand.node],
                        operand.value_type,
                        operand.stage,
                    ),
                }
            }
            Expression::Binary {
                operator,
                left,
                right,
            } => {
                let left = self.compile(left)?;
                let right = self.compile(right)?;
                self.compile_binary(*operator, left, right)
            }
            Expression::Call {
                function,
                arguments,
            } => {
                let arguments = arguments
                    .iter()
                    .map(|argument| self.compile(argument))
                    .collect::<Result<Vec<_>, _>>()?;
                self.compile_call(function, arguments)
            }
        }
    }

    fn compile_binary(
        &mut self,
        operator: BinaryOperator,
        mut left: Binding,
        mut right: Binding,
    ) -> Result<Binding, ExpressionError> {
        let stage = left.stage.max(right.stage);
        match operator {
            BinaryOperator::Add | BinaryOperator::Subtract => {
                if left.value_type.dimension != right.value_type.dimension {
                    return Err(ExpressionError::TypeMismatch(
                        "addition and subtraction require equal dimensions".into(),
                    ));
                }
                right = self.convert(right, &left.value_type.unit)?;
                let (shape, stagger) = broadcast(&left.value_type, &right.value_type)?;
                let output_type = ValueType::new(left.value_type.unit.clone(), shape, stagger)
                    .map_err(ExpressionError::Internal)?;
                self.push(
                    if operator == BinaryOperator::Add {
                        GraphOp::Add
                    } else {
                        GraphOp::Subtract
                    },
                    vec![left.node, right.node],
                    output_type,
                    stage,
                )
            }
            BinaryOperator::Multiply | BinaryOperator::Divide => {
                left = self.normalize_to_si(left)?;
                right = self.normalize_to_si(right)?;
                let dimension = if operator == BinaryOperator::Multiply {
                    left.value_type
                        .dimension
                        .checked_product(right.value_type.dimension)
                } else {
                    left.value_type
                        .dimension
                        .checked_quotient(right.value_type.dimension)
                }
                .ok_or_else(|| {
                    ExpressionError::TypeMismatch("dimension exponent overflow".into())
                })?;
                let (shape, stagger) = broadcast(&left.value_type, &right.value_type)?;
                let output_type =
                    ValueType::new(GraphUnit::si_for_dimension(dimension), shape, stagger)
                        .map_err(ExpressionError::Internal)?;
                self.push(
                    if operator == BinaryOperator::Multiply {
                        GraphOp::Multiply
                    } else {
                        GraphOp::Divide
                    },
                    vec![left.node, right.node],
                    output_type,
                    stage,
                )
            }
        }
    }

    fn compile_call(
        &mut self,
        function: &str,
        arguments: Vec<Binding>,
    ) -> Result<Binding, ExpressionError> {
        match function {
            "deaccumulate" => {
                expect_function_arity(function, &arguments, 1)?;
                let input = arguments[0].clone();
                self.push(
                    GraphOp::Deaccumulate,
                    vec![input.node],
                    input.value_type,
                    input.stage.max(ExecutionStage::Frame),
                )
            }
            "interval_seconds" => {
                expect_function_arity(function, &arguments, 0)?;
                let output_type = ValueType::new(
                    GraphUnit::parse("s").map_err(ExpressionError::Unit)?,
                    FieldShape::Scalar,
                    None,
                )
                .map_err(ExpressionError::Internal)?;
                self.push(
                    GraphOp::IntervalSeconds,
                    Vec::new(),
                    output_type,
                    ExecutionStage::Frame,
                )
            }
            "virtual_temperature" => self.thermodynamic(
                function,
                arguments,
                ThermodynamicOp::VirtualTemperature,
                "K",
            ),
            "air_density" => {
                self.thermodynamic(function, arguments, ThermodynamicOp::AirDensity, "kg/m3")
            }
            "relative_humidity" => {
                self.thermodynamic(function, arguments, ThermodynamicOp::RelativeHumidity, "1")
            }
            "potential_temperature" => self.thermodynamic(
                function,
                arguments,
                ThermodynamicOp::PotentialTemperature,
                "K",
            ),
            "geopotential_height" => self.thermodynamic(
                function,
                arguments,
                ThermodynamicOp::GeopotentialHeight,
                "m",
            ),
            "geopotential_from_height" => self.thermodynamic(
                function,
                arguments,
                ThermodynamicOp::GeopotentialFromHeight,
                "m2/s2",
            ),
            "surface_pressure_from_log" => self.thermodynamic(
                function,
                arguments,
                ThermodynamicOp::SurfacePressureFromLog,
                "Pa",
            ),
            "two_metre_specific_humidity_from_dewpoint" => self.thermodynamic(
                function,
                arguments,
                ThermodynamicOp::TwoMetreSpecificHumidityFromDewpoint,
                "1",
            ),
            "latent_heat_from_moisture_flux" => self.thermodynamic(
                function,
                arguments,
                ThermodynamicOp::LatentHeatFromMoistureFlux,
                "kg/s3",
            ),
            "hybrid_interface_pressure" => self.vertical(
                function,
                arguments,
                VerticalOp::HybridInterfacePressure,
                FieldShape::Interface3D,
                VerticalStagger::Interface,
            ),
            "hybrid_full_pressure" => self.vertical(
                function,
                arguments,
                VerticalOp::HybridFullPressure,
                FieldShape::Full3D,
                VerticalStagger::Full,
            ),
            _ => Err(ExpressionError::UnknownFunction(function.into())),
        }
    }

    fn thermodynamic(
        &mut self,
        function: &str,
        arguments: Vec<Binding>,
        operation: ThermodynamicOp,
        output_unit: &str,
    ) -> Result<Binding, ExpressionError> {
        let expected = match operation {
            ThermodynamicOp::VirtualTemperature => 2,
            ThermodynamicOp::AirDensity => 2,
            ThermodynamicOp::RelativeHumidity => 3,
            ThermodynamicOp::PotentialTemperature => 2,
            ThermodynamicOp::GeopotentialHeight => 1,
            ThermodynamicOp::GeopotentialFromHeight => 1,
            ThermodynamicOp::SurfacePressureFromLog => 1,
            ThermodynamicOp::TwoMetreSpecificHumidityFromDewpoint => 2,
            ThermodynamicOp::LatentHeatFromMoistureFlux => 1,
        };
        expect_function_arity(function, &arguments, expected)?;
        let (shape, stagger) = broadcast_bindings(&arguments)?;
        let stage = arguments
            .iter()
            .map(|argument| argument.stage)
            .max()
            .unwrap_or(ExecutionStage::Frame);
        let output_type = ValueType::new(
            GraphUnit::parse(output_unit).map_err(ExpressionError::Unit)?,
            shape,
            stagger,
        )
        .map_err(ExpressionError::Internal)?;
        self.push(
            GraphOp::Thermodynamic(operation),
            arguments
                .into_iter()
                .map(|argument| argument.node)
                .collect(),
            output_type,
            stage,
        )
    }

    fn vertical(
        &mut self,
        function: &str,
        arguments: Vec<Binding>,
        operation: VerticalOp,
        shape: FieldShape,
        stagger: VerticalStagger,
    ) -> Result<Binding, ExpressionError> {
        expect_function_arity(function, &arguments, 1)?;
        let output_type = ValueType::new(
            GraphUnit::parse("Pa").map_err(ExpressionError::Unit)?,
            shape,
            Some(stagger),
        )
        .map_err(ExpressionError::Internal)?;
        self.push(
            GraphOp::Vertical(operation),
            vec![arguments[0].node.clone()],
            output_type,
            ExecutionStage::Column.max(arguments[0].stage),
        )
    }

    fn convert(
        &mut self,
        binding: Binding,
        target: &GraphUnit,
    ) -> Result<Binding, ExpressionError> {
        if binding.value_type.dimension != target.dimension() {
            return Err(ExpressionError::TypeMismatch(format!(
                "cannot convert unit '{}' to '{}'",
                binding.value_type.unit.symbol(),
                target.symbol()
            )));
        }
        if binding.value_type.unit == *target {
            return Ok(binding);
        }
        let output_type = ValueType::new(
            target.clone(),
            binding.value_type.shape,
            binding.value_type.vertical_stagger,
        )
        .map_err(ExpressionError::Internal)?;
        self.push(
            GraphOp::ConvertUnit(target.clone()),
            vec![binding.node],
            output_type,
            binding.stage,
        )
    }

    fn normalize_to_si(&mut self, binding: Binding) -> Result<Binding, ExpressionError> {
        let target = GraphUnit::si_for_dimension(binding.value_type.dimension);
        self.convert(binding, &target)
    }

    fn push(
        &mut self,
        operation: GraphOp,
        inputs: Vec<GraphNodeId>,
        value_type: ValueType,
        stage: ExecutionStage,
    ) -> Result<Binding, ExpressionError> {
        let id = GraphNodeId(format!("expression:{:016x}", self.next_node));
        self.next_node = self
            .next_node
            .checked_add(1)
            .ok_or_else(|| ExpressionError::Internal("expression node counter overflow".into()))?;
        self.nodes.push(GraphNode {
            id: id.clone(),
            operation,
            inputs,
            output_type: value_type.clone(),
            stage,
        });
        Ok(Binding {
            node: id,
            value_type,
            stage,
        })
    }
}

fn expect_function_arity(
    function: &str,
    arguments: &[Binding],
    expected: usize,
) -> Result<(), ExpressionError> {
    if arguments.len() != expected {
        return Err(ExpressionError::WrongFunctionArity {
            function: function.into(),
            expected,
            actual: arguments.len(),
        });
    }
    Ok(())
}

fn broadcast_bindings(
    bindings: &[Binding],
) -> Result<(FieldShape, Option<VerticalStagger>), ExpressionError> {
    let first = bindings.first().ok_or_else(|| {
        ExpressionError::Internal("cannot broadcast an empty argument list".into())
    })?;
    bindings.iter().skip(1).try_fold(
        (first.value_type.shape, first.value_type.vertical_stagger),
        |(shape, stagger), binding| {
            let left = ValueType {
                unit: GraphUnit::dimensionless(),
                dimension: DimensionVector::DIMENSIONLESS,
                shape,
                vertical_stagger: stagger,
            };
            broadcast(&left, &binding.value_type)
        },
    )
}

fn broadcast(
    left: &ValueType,
    right: &ValueType,
) -> Result<(FieldShape, Option<VerticalStagger>), ExpressionError> {
    match (left.shape, right.shape) {
        (FieldShape::Scalar, _) => Ok((right.shape, right.vertical_stagger)),
        (_, FieldShape::Scalar) => Ok((left.shape, left.vertical_stagger)),
        (left_shape, right_shape)
            if left_shape == right_shape && left.vertical_stagger == right.vertical_stagger =>
        {
            Ok((left_shape, left.vertical_stagger))
        }
        _ => Err(ExpressionError::TypeMismatch(format!(
            "cannot broadcast {:?}/{:?} with {:?}/{:?}",
            left.shape, left.vertical_stagger, right.shape, right.vertical_stagger
        ))),
    }
}

fn stagger_for_shape(shape: FieldShape) -> Option<VerticalStagger> {
    match shape {
        FieldShape::Scalar | FieldShape::Horizontal2D => None,
        FieldShape::Full3D => Some(VerticalStagger::Full),
        FieldShape::Interface3D => Some(VerticalStagger::Interface),
    }
}

fn extension_value_types(
    descriptors: &[ExtensionFieldDescriptor],
) -> Result<BTreeMap<FieldReference, ValueType>, ExpressionError> {
    let mut types = BTreeMap::new();
    for descriptor in descriptors {
        if !matches!(descriptor.target, FieldReference::Extension { .. }) {
            return Err(ExpressionError::InvalidExtensionDescriptor {
                target: descriptor.target.clone(),
                message: "descriptor target must be a namespaced extension field".into(),
            });
        }
        if descriptor.unit.trim().is_empty() {
            return Err(ExpressionError::InvalidExtensionDescriptor {
                target: descriptor.target.clone(),
                message: "unit must not be empty".into(),
            });
        }
        let unit = GraphUnit::parse(&descriptor.unit).map_err(|error| {
            ExpressionError::InvalidExtensionDescriptor {
                target: descriptor.target.clone(),
                message: format!("invalid unit '{}': {error}", descriptor.unit),
            }
        })?;
        let value_type =
            ValueType::new(unit, descriptor.shape, stagger_for_shape(descriptor.shape)).map_err(
                |message| ExpressionError::InvalidExtensionDescriptor {
                    target: descriptor.target.clone(),
                    message,
                },
            )?;
        if types
            .insert(descriptor.target.clone(), value_type)
            .is_some()
        {
            return Err(ExpressionError::DuplicateExtensionDescriptor(
                descriptor.target.clone(),
            ));
        }
    }
    Ok(types)
}

fn registered_value_type(
    reference: &FieldReference,
    extension_types: &BTreeMap<FieldReference, ValueType>,
) -> Result<ValueType, ExpressionError> {
    match reference {
        FieldReference::Canonical(field) => canonical_value_type(*field),
        FieldReference::Extension { .. } => extension_types
            .get(reference)
            .cloned()
            .ok_or_else(|| ExpressionError::ExtensionTypeRequired(reference.clone())),
    }
}

fn canonical_value_type(field: CanonicalField) -> Result<ValueType, ExpressionError> {
    let semantics = field.semantics();
    ValueType::new(
        GraphUnit::parse(semantics.unit).map_err(ExpressionError::Unit)?,
        semantics.shape,
        semantics.vertical_stagger,
    )
    .map_err(ExpressionError::Internal)
}

#[derive(Clone, Debug, PartialEq)]
enum TokenKind {
    Identifier(String),
    Number(f64),
    Plus,
    Minus,
    Star,
    Slash,
    LeftParen,
    RightParen,
    Comma,
    End,
}

#[derive(Clone, Debug, PartialEq)]
struct Token {
    kind: TokenKind,
    offset: usize,
}

struct Lexer<'a> {
    input: &'a str,
    offset: usize,
}

impl<'a> Lexer<'a> {
    const fn new(input: &'a str) -> Self {
        Self { input, offset: 0 }
    }

    fn tokenize(mut self) -> Result<Vec<Token>, ExpressionError> {
        let mut tokens = Vec::new();
        while self.offset < self.input.len() {
            let character = self.current_char().ok_or_else(|| ExpressionError::Syntax {
                offset: self.offset,
                message: "invalid UTF-8 boundary".into(),
            })?;
            if character.is_whitespace() {
                self.offset += character.len_utf8();
                continue;
            }
            let offset = self.offset;
            let kind = match character {
                '+' => {
                    self.offset += 1;
                    TokenKind::Plus
                }
                '-' => {
                    self.offset += 1;
                    TokenKind::Minus
                }
                '*' => {
                    self.offset += 1;
                    TokenKind::Star
                }
                '/' => {
                    self.offset += 1;
                    TokenKind::Slash
                }
                '(' => {
                    self.offset += 1;
                    TokenKind::LeftParen
                }
                ')' => {
                    self.offset += 1;
                    TokenKind::RightParen
                }
                ',' => {
                    self.offset += 1;
                    TokenKind::Comma
                }
                value if value.is_ascii_digit() || value == '.' => self.number()?,
                value if value.is_ascii_alphabetic() || value == '_' => self.identifier(),
                _ => {
                    return Err(ExpressionError::Syntax {
                        offset,
                        message: format!("unexpected character '{character}'"),
                    });
                }
            };
            tokens.push(Token { kind, offset });
            if tokens.len() > MAX_TOKENS {
                return Err(ExpressionError::TooComplex);
            }
        }
        tokens.push(Token {
            kind: TokenKind::End,
            offset: self.input.len(),
        });
        Ok(tokens)
    }

    fn current_char(&self) -> Option<char> {
        self.input.get(self.offset..)?.chars().next()
    }

    fn identifier(&mut self) -> TokenKind {
        let start = self.offset;
        while let Some(character) = self.current_char() {
            if !character.is_ascii_alphanumeric() && character != '_' {
                break;
            }
            self.offset += character.len_utf8();
        }
        TokenKind::Identifier(self.input[start..self.offset].into())
    }

    fn number(&mut self) -> Result<TokenKind, ExpressionError> {
        let start = self.offset;
        let bytes = self.input.as_bytes();
        let mut saw_digit = false;
        while self.offset < bytes.len() && bytes[self.offset].is_ascii_digit() {
            saw_digit = true;
            self.offset += 1;
        }
        if self.offset < bytes.len() && bytes[self.offset] == b'.' {
            self.offset += 1;
            while self.offset < bytes.len() && bytes[self.offset].is_ascii_digit() {
                saw_digit = true;
                self.offset += 1;
            }
        }
        if !saw_digit {
            return Err(ExpressionError::Syntax {
                offset: start,
                message: "a decimal point must contain at least one digit".into(),
            });
        }
        if self.offset < bytes.len() && matches!(bytes[self.offset], b'e' | b'E') {
            self.offset += 1;
            if self.offset < bytes.len() && matches!(bytes[self.offset], b'+' | b'-') {
                self.offset += 1;
            }
            let exponent_start = self.offset;
            while self.offset < bytes.len() && bytes[self.offset].is_ascii_digit() {
                self.offset += 1;
            }
            if self.offset == exponent_start {
                return Err(ExpressionError::Syntax {
                    offset: start,
                    message: "scientific notation requires an exponent".into(),
                });
            }
        }
        let text = &self.input[start..self.offset];
        let value = text.parse::<f64>().map_err(|_| ExpressionError::Syntax {
            offset: start,
            message: format!("invalid number '{text}'"),
        })?;
        if !value.is_finite() {
            return Err(ExpressionError::NonFiniteConstant { offset: start });
        }
        Ok(TokenKind::Number(value))
    }
}

struct Parser {
    tokens: Vec<Token>,
    cursor: usize,
    nesting: usize,
}

impl Parser {
    const fn new(tokens: Vec<Token>) -> Self {
        Self {
            tokens,
            cursor: 0,
            nesting: 0,
        }
    }

    fn parse(mut self) -> Result<Expression, ExpressionError> {
        let expression = self.parse_binding_power(0)?;
        if !matches!(self.current().kind, TokenKind::End) {
            return Err(self.syntax("unexpected token after expression"));
        }
        Ok(expression)
    }

    fn parse_binding_power(&mut self, minimum: u8) -> Result<Expression, ExpressionError> {
        self.nesting += 1;
        if self.nesting > MAX_NESTING {
            return Err(ExpressionError::TooComplex);
        }
        let mut left = self.parse_prefix()?;
        loop {
            let (operator, left_power, right_power) = match self.current().kind {
                TokenKind::Plus => (BinaryOperator::Add, 1, 2),
                TokenKind::Minus => (BinaryOperator::Subtract, 1, 2),
                TokenKind::Star => (BinaryOperator::Multiply, 3, 4),
                TokenKind::Slash => (BinaryOperator::Divide, 3, 4),
                _ => break,
            };
            if left_power < minimum {
                break;
            }
            self.cursor += 1;
            let right = self.parse_binding_power(right_power)?;
            left = Expression::Binary {
                operator,
                left: Box::new(left),
                right: Box::new(right),
            };
        }
        self.nesting -= 1;
        Ok(left)
    }

    fn parse_prefix(&mut self) -> Result<Expression, ExpressionError> {
        let token = self.current().clone();
        match token.kind {
            TokenKind::Number(value) => {
                self.cursor += 1;
                Ok(Expression::Constant(value))
            }
            TokenKind::Identifier(identifier) => {
                self.cursor += 1;
                if matches!(self.current().kind, TokenKind::LeftParen) {
                    self.parse_call(identifier)
                } else {
                    Ok(Expression::Identifier(identifier))
                }
            }
            TokenKind::Minus => {
                self.cursor += 1;
                Ok(Expression::Unary {
                    operator: UnaryOperator::Negate,
                    operand: Box::new(self.parse_binding_power(5)?),
                })
            }
            TokenKind::LeftParen => {
                self.cursor += 1;
                let expression = self.parse_binding_power(0)?;
                self.expect_right_parenthesis()?;
                Ok(expression)
            }
            _ => Err(ExpressionError::Syntax {
                offset: token.offset,
                message: "expected an identifier, number, unary '-', or '('".into(),
            }),
        }
    }

    fn parse_call(&mut self, function: String) -> Result<Expression, ExpressionError> {
        self.cursor += 1;
        let mut arguments = Vec::new();
        if matches!(self.current().kind, TokenKind::RightParen) {
            self.cursor += 1;
            return Ok(Expression::Call {
                function,
                arguments,
            });
        }
        loop {
            arguments.push(self.parse_binding_power(0)?);
            match self.current().kind {
                TokenKind::Comma => self.cursor += 1,
                TokenKind::RightParen => {
                    self.cursor += 1;
                    break;
                }
                _ => return Err(self.syntax("expected ',' or ')' in function arguments")),
            }
        }
        Ok(Expression::Call {
            function,
            arguments,
        })
    }

    fn expect_right_parenthesis(&mut self) -> Result<(), ExpressionError> {
        if !matches!(self.current().kind, TokenKind::RightParen) {
            return Err(self.syntax("expected ')'"));
        }
        self.cursor += 1;
        Ok(())
    }

    fn current(&self) -> &Token {
        let last = self.tokens.len().saturating_sub(1);
        &self.tokens[self.cursor.min(last)]
    }

    fn syntax(&self, message: &str) -> ExpressionError {
        ExpressionError::Syntax {
            offset: self.current().offset,
            message: message.into(),
        }
    }
}

/// Profile expression parse, type-check, or graph-compilation failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExpressionError {
    /// Lexical or syntactic error at a UTF-8 byte offset.
    Syntax {
        /// Byte offset in the source expression.
        offset: usize,
        /// Stable explanation.
        message: String,
    },
    /// A parsed constant was NaN or infinite.
    NonFiniteConstant {
        /// Byte offset in the source expression.
        offset: usize,
    },
    /// The expression exceeds v0 complexity limits.
    TooComplex,
    /// A symbolic dependency is absent.
    UnknownIdentifier(String),
    /// A function is outside the whitelist.
    UnknownFunction(String),
    /// A whitelisted function has the wrong argument count.
    WrongFunctionArity {
        /// Function name.
        function: String,
        /// Required argument count.
        expected: usize,
        /// Actual argument count.
        actual: usize,
    },
    /// Physical dimensions, shapes, or staggering are incompatible.
    TypeMismatch(String),
    /// Derived fields form a dependency cycle.
    DependencyCycle(Vec<String>),
    /// A field declares an execution stage earlier than its expression permits.
    StageTooEarly {
        /// Derived field identifier.
        field: String,
        /// Earliest legal stage.
        required: ExecutionStage,
        /// Declared stage.
        declared: ExecutionStage,
    },
    /// A source alternative declares an incompatible unit.
    InvalidSourceUnit {
        /// Mapping identifier.
        field: String,
        /// Unit text.
        unit: String,
        /// Stable explanation.
        message: String,
    },
    /// A derived output unit cannot be parsed.
    InvalidDeclaredUnit {
        /// Derived field identifier.
        field: String,
        /// Unit text.
        unit: String,
        /// Stable explanation.
        message: String,
    },
    /// A derived shape/stagger declaration is internally invalid.
    InvalidDeclaredType {
        /// Derived field identifier.
        field: String,
        /// Stable explanation.
        message: String,
    },
    /// An expression disagrees with its declared output type.
    DeclaredTypeMismatch {
        /// Derived field identifier.
        field: String,
        /// Stable explanation.
        message: String,
    },
    /// A derived declaration disagrees with canonical target semantics.
    CanonicalTypeMismatch {
        /// Derived field identifier.
        field: String,
        /// Target field.
        target: FieldReference,
    },
    /// Extension fields need a separately registered descriptor before compilation.
    ExtensionTypeRequired(FieldReference),
    /// An extension descriptor is malformed or has an invalid unit/shape.
    InvalidExtensionDescriptor {
        /// Descriptor target.
        target: FieldReference,
        /// Stable explanation.
        message: String,
    },
    /// One extension field was described more than once.
    DuplicateExtensionDescriptor(FieldReference),
    /// A newly added canonical field has no v0 descriptor yet.
    UnsupportedCanonicalField(CanonicalField),
    /// Error scoped to one derived field during parsing.
    InDerivedField {
        /// Derived field identifier.
        field: String,
        /// Stable explanation.
        message: String,
    },
    /// Unit parser failure.
    Unit(UnitParseError),
    /// Final graph validation failure.
    Graph(GraphError),
    /// Internal checked invariant failure.
    Internal(String),
}

impl fmt::Display for ExpressionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Syntax { offset, message } => {
                write!(
                    formatter,
                    "expression syntax error at byte {offset}: {message}"
                )
            }
            Self::NonFiniteConstant { offset } => {
                write!(formatter, "non-finite constant at byte {offset}")
            }
            Self::TooComplex => formatter.write_str("expression exceeds v0 complexity limits"),
            Self::UnknownIdentifier(identifier) => {
                write!(formatter, "unknown expression identifier '{identifier}'")
            }
            Self::UnknownFunction(function) => {
                write!(formatter, "unknown Profile function '{function}'")
            }
            Self::WrongFunctionArity {
                function,
                expected,
                actual,
            } => write!(
                formatter,
                "function '{function}' expects {expected} arguments but has {actual}"
            ),
            Self::TypeMismatch(message) => write!(formatter, "expression type mismatch: {message}"),
            Self::DependencyCycle(fields) => {
                write!(
                    formatter,
                    "derived-field cycle contains {}",
                    fields.join(", ")
                )
            }
            Self::StageTooEarly {
                field,
                required,
                declared,
            } => write!(
                formatter,
                "derived field '{field}' requires {required:?} but declares {declared:?}"
            ),
            Self::InvalidSourceUnit {
                field,
                unit,
                message,
            } => write!(
                formatter,
                "source field '{field}' has invalid unit '{unit}': {message}"
            ),
            Self::InvalidDeclaredUnit {
                field,
                unit,
                message,
            } => write!(
                formatter,
                "derived field '{field}' has invalid unit '{unit}': {message}"
            ),
            Self::InvalidDeclaredType { field, message }
            | Self::DeclaredTypeMismatch { field, message } => {
                write!(
                    formatter,
                    "derived field '{field}' has invalid type: {message}"
                )
            }
            Self::CanonicalTypeMismatch { field, target } => write!(
                formatter,
                "derived field '{field}' is incompatible with canonical target {target:?}"
            ),
            Self::ExtensionTypeRequired(target) => write!(
                formatter,
                "extension target {target:?} requires a registered descriptor"
            ),
            Self::InvalidExtensionDescriptor { target, message } => write!(
                formatter,
                "extension descriptor for {target:?} is invalid: {message}"
            ),
            Self::DuplicateExtensionDescriptor(target) => write!(
                formatter,
                "extension target {target:?} has more than one descriptor"
            ),
            Self::UnsupportedCanonicalField(field) => {
                write!(
                    formatter,
                    "canonical field {field:?} has no v0 graph descriptor"
                )
            }
            Self::InDerivedField { field, message } => {
                write!(formatter, "derived field '{field}': {message}")
            }
            Self::Unit(error) => error.fmt(formatter),
            Self::Graph(error) => error.fmt(formatter),
            Self::Internal(message) => {
                write!(formatter, "internal expression invariant: {message}")
            }
        }
    }
}

impl std::error::Error for ExpressionError {}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use crate::field::{FieldQuality, FieldShape};
    use crate::profile::document::{
        DatasetFingerprint, DatasetProfileDocument, DerivedField, ExtensionFieldDescriptor,
        FieldMapping, FieldSource, GribSourceMatcher, PROFILE_SCHEMA_VERSION, ProfileDocumentKind,
        ProfileName, SourceMatcher, TemporalSemantics,
    };

    use super::*;

    #[test]
    fn parser_obeys_precedence_and_rejects_external_syntax() {
        let expression = parse_expression("a + 2 * -(b - 3e-2)").unwrap();
        let Expression::Binary {
            operator: BinaryOperator::Add,
            right,
            ..
        } = expression
        else {
            panic!("expected addition root");
        };
        assert!(matches!(
            *right,
            Expression::Binary {
                operator: BinaryOperator::Multiply,
                ..
            }
        ));
        assert!(parse_expression("read_file('x')").is_err());
        assert!(parse_expression("a; system()").is_err());
    }

    fn base_profile() -> DatasetProfileDocument {
        DatasetProfileDocument {
            schema_version: PROFILE_SCHEMA_VERSION,
            kind: ProfileDocumentKind::DatasetProfile,
            name: ProfileName("typed-test".into()),
            fingerprint: DatasetFingerprint::default(),
            source_matchers: vec![SourceMatcher::Grib1 {
                matcher: GribSourceMatcher {
                    centre: Some(98),
                    ..GribSourceMatcher::default()
                },
            }],
            candidate_path_globs: Vec::new(),
            frame_interval_seconds: None,
            extension_fields: Vec::new(),
            fields: vec![
                FieldMapping {
                    id: "temperature".into(),
                    target: FieldReference::Canonical(CanonicalField::AirTemperature),
                    sources: vec![FieldSource {
                        identity: BTreeMap::from([("parameter".into(), "130".into())]),
                        unit: "K".into(),
                        role: crate::profile::document::SourceRole::Primary,
                        quality: FieldQuality::Source,
                    }],
                    temporal: TemporalSemantics::default(),
                },
                FieldMapping {
                    id: "surface_pressure".into(),
                    target: FieldReference::Canonical(CanonicalField::SurfacePressure),
                    sources: vec![FieldSource {
                        identity: BTreeMap::from([("parameter".into(), "134".into())]),
                        unit: "Pa".into(),
                        role: crate::profile::document::SourceRole::Primary,
                        quality: FieldQuality::Source,
                    }],
                    temporal: TemporalSemantics::default(),
                },
            ],
            derived_fields: vec![DerivedField {
                id: "density".into(),
                target: FieldReference::Canonical(CanonicalField::AirDensity),
                expression: "air_density(hybrid_full_pressure(surface_pressure), temperature)"
                    .into(),
                unit: "kg/m3".into(),
                shape: FieldShape::Full3D,
                stage: ExecutionStage::Column,
                quality: FieldQuality::Derived,
                temporal: TemporalSemantics::default(),
            }],
            capabilities: BTreeMap::new(),
        }
    }

    #[test]
    fn profile_compilation_checks_units_shapes_and_dependencies() {
        let mut profile = base_profile();
        let compiled = compile_profile_graph(&profile).unwrap();
        assert_eq!(compiled.graph.outputs.len(), 3);
        assert_eq!(compiled.execution_plan.stages.len(), 2);

        profile.derived_fields[0].target =
            FieldReference::Canonical(CanonicalField::GeopotentialHeight);
        assert!(matches!(
            compile_profile_graph(&profile),
            Err(ExpressionError::CanonicalTypeMismatch { .. })
        ));

        profile.derived_fields[0].target =
            FieldReference::Canonical(CanonicalField::AirTemperature);
        profile.fields[0].target = FieldReference::Canonical(CanonicalField::GeopotentialHeight);
        assert!(matches!(
            compile_profile_graph(&profile),
            Err(ExpressionError::InvalidSourceUnit { .. })
        ));
    }

    #[test]
    fn derived_cycles_are_rejected_before_graph_construction() {
        let mut profile = base_profile();
        profile.derived_fields = vec![
            DerivedField {
                id: "a".into(),
                expression: "b".into(),
                ..profile.derived_fields[0].clone()
            },
            DerivedField {
                id: "b".into(),
                expression: "a".into(),
                ..profile.derived_fields[0].clone()
            },
        ];
        assert!(matches!(
            compile_profile_graph(&profile),
            Err(ExpressionError::DependencyCycle(_))
        ));
    }

    #[test]
    fn direct_extensions_require_explicit_types() {
        let mut profile = base_profile();
        let extension = FieldReference::Extension {
            namespace: "test".into(),
            name: "height".into(),
        };
        profile.fields[0].target = extension.clone();
        assert_eq!(
            compile_profile_graph(&profile),
            Err(ExpressionError::ExtensionTypeRequired(extension.clone()))
        );

        profile.extension_fields.push(ExtensionFieldDescriptor {
            target: extension,
            unit: "K".into(),
            shape: FieldShape::Full3D,
        });
        assert!(compile_profile_graph(&profile).is_ok());
    }

    #[test]
    fn registered_derived_extension_must_match_its_descriptor() {
        let mut profile = base_profile();
        let extension = FieldReference::Extension {
            namespace: "test".into(),
            name: "density".into(),
        };
        profile.derived_fields[0].target = extension.clone();
        profile.extension_fields.push(ExtensionFieldDescriptor {
            target: extension,
            unit: "m".into(),
            shape: FieldShape::Full3D,
        });
        assert!(matches!(
            compile_profile_graph(&profile),
            Err(ExpressionError::CanonicalTypeMismatch { .. })
        ));
    }
}
