//! Typed relational IR values and conservative lowering outcomes.
//!
//! This module defines the value contract only. Query parsing, lowering,
//! normalization, comparison, formatting, and front-end presentation belong to
//! later layers. In particular, facts that cannot be proved are represented as
//! [`Knowledge::Unknown`] rather than inferred from a member multiplicity.

use std::collections::BTreeSet;

use pure_analyzer_diagnostics::{FileId, ReasonCode, TextRange};
use pure_analyzer_model::{Multiplicity, Name, Provenance, QName, TypeRef};
use pure_analyzer_resolve::{
    DefinitionAnchor, LocalValue, LocalValueKind, NavigationChain, NavigationTarget, ResolvedClass,
    ResolvedMember, ResolvedMemberKind,
};

const BOOLEAN_TYPE: &str = "Boolean";
const EXACTLY_ONE: u32 = 1;
const INTEGER_TYPE: &str = "Integer";
const STRING_TYPE: &str = "String";

/// A stable range in a query document loaded by the calling front end.
///
/// This is intentionally distinct from [`DefinitionAnchor`]: query files are
/// request-local front-end inputs, while model anchors identify the source that
/// supplied a resolved model fact.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SourceSpan {
    file: FileId,
    range: TextRange,
}

impl SourceSpan {
    /// Construct a query-document source span.
    #[must_use]
    pub const fn new(file: FileId, range: TextRange) -> Self {
        Self { file, range }
    }

    /// Return the request-local file that owns this span.
    #[must_use]
    pub const fn file(self) -> FileId {
        self.file
    }

    /// Return the byte range within [`Self::file`].
    #[must_use]
    pub const fn range(self) -> TextRange {
        self.range
    }
}

/// The model definition category that contributed an IR provenance fact.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelOriginKind {
    /// A resolved class supplied the fact.
    Class,
    /// A resolved property or association end supplied the fact.
    Member,
    /// A caller supplied an anchor without a more specific category.
    Unspecified,
}

/// A resolved model definition that contributed to an IR value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelOrigin {
    kind: ModelOriginKind,
    provenance: Provenance,
    definition: DefinitionAnchor,
    identity: ModelOriginIdentity,
}

/// Graph-level identity retained when a source anchor is not precise enough.
///
/// PMCD definitions currently share a document-level anchor without element
/// spans. The path/name identity prevents unrelated resolved definitions from
/// collapsing when IR provenance is merged.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ModelOriginIdentity {
    Unspecified,
    Class(QName),
    Member {
        owner: QName,
        name: Name,
        kind: ResolvedMemberKind,
    },
}

impl ModelOrigin {
    /// Construct an uncategorized model-origin fact from its provenance and anchor.
    #[must_use]
    pub const fn new(provenance: Provenance, definition: DefinitionAnchor) -> Self {
        Self {
            kind: ModelOriginKind::Unspecified,
            provenance,
            definition,
            identity: ModelOriginIdentity::Unspecified,
        }
    }

    /// Construct an origin from a resolved class.
    #[must_use]
    pub fn from_class(class: &ResolvedClass) -> Self {
        Self {
            kind: ModelOriginKind::Class,
            provenance: class.provenance(),
            definition: class.definition(),
            identity: ModelOriginIdentity::Class(class.path().clone()),
        }
    }

    /// Construct an origin from a resolved member.
    #[must_use]
    pub fn from_member(member: &ResolvedMember) -> Self {
        Self {
            kind: ModelOriginKind::Member,
            provenance: member.provenance(),
            definition: member.definition(),
            identity: ModelOriginIdentity::Member {
                owner: member.owner().path().clone(),
                name: member.name().clone(),
                kind: member.kind().clone(),
            },
        }
    }

    /// Return the definition category that supplied this fact.
    #[must_use]
    pub const fn kind(&self) -> ModelOriginKind {
        self.kind
    }

    /// Return the source kind that supplied the model fact.
    #[must_use]
    pub const fn provenance(&self) -> Provenance {
        self.provenance
    }

    /// Return the resolved definition's source and optional span.
    #[must_use]
    pub const fn definition(&self) -> DefinitionAnchor {
        self.definition
    }
}

/// Combined query and model provenance for an IR value or proven fact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IrOrigin {
    source: SourceSpan,
    model_origins: Vec<ModelOrigin>,
}

impl IrOrigin {
    /// Construct an origin with the query span and contributing model facts.
    #[must_use]
    pub fn new(source: SourceSpan, model_origins: Vec<ModelOrigin>) -> Self {
        Self {
            source,
            model_origins,
        }
    }

    /// Return the source span in the query being analyzed.
    #[must_use]
    pub const fn source(&self) -> SourceSpan {
        self.source
    }

    /// Return model facts in the deterministic order supplied by lowering.
    #[must_use]
    pub fn model_origins(&self) -> &[ModelOrigin] {
        &self.model_origins
    }
}

/// Stable identity for one output column, independent of its display name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ColumnId(u32);

impl ColumnId {
    /// Construct a stable column identity chosen by the lowering operation.
    #[must_use]
    pub const fn new(index: u32) -> Self {
        Self(index)
    }

    /// Return the raw stable column identity.
    #[must_use]
    pub const fn index(self) -> u32 {
        self.0
    }
}

/// What the available facts establish about a column's nullability.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Nullability {
    /// The column is proven never to contain `null`.
    NonNullable,
    /// The column is proven able to contain `null`.
    Nullable,
    /// The available facts do not establish nullability.
    Unknown,
}

/// One ordered column in a relation schema.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Column {
    id: ColumnId,
    name: Name,
    type_ref: TypeRef,
    multiplicity: Multiplicity,
    nullability: Nullability,
    origin: IrOrigin,
}

impl Column {
    /// Construct a column with all semantic facts explicit.
    #[must_use]
    pub fn new(
        id: ColumnId,
        name: Name,
        type_ref: TypeRef,
        multiplicity: Multiplicity,
        nullability: Nullability,
        origin: IrOrigin,
    ) -> Self {
        Self {
            id,
            name,
            type_ref,
            multiplicity,
            nullability,
            origin,
        }
    }

    /// Return the stable identity, which remains distinct from the column name.
    #[must_use]
    pub const fn id(&self) -> ColumnId {
        self.id
    }

    /// Return the display name or alias in this schema.
    #[must_use]
    pub const fn name(&self) -> &Name {
        &self.name
    }

    /// Return the declared Pure type.
    #[must_use]
    pub const fn type_ref(&self) -> &TypeRef {
        &self.type_ref
    }

    /// Return the value multiplicity without deriving any relational fact from it.
    #[must_use]
    pub const fn multiplicity(&self) -> Multiplicity {
        self.multiplicity
    }

    /// Return explicit nullability knowledge for this column.
    #[must_use]
    pub const fn nullability(&self) -> Nullability {
        self.nullability
    }

    /// Return query and model provenance for this column.
    #[must_use]
    pub const fn origin(&self) -> &IrOrigin {
        &self.origin
    }
}

/// A schema construction invariant failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SchemaError {
    /// Two columns were assigned the same stable identity.
    #[error("relation schema has duplicate column identity")]
    DuplicateColumnId(ColumnId),
}

/// Explicit, ordered output schema for one relational expression.
///
/// A `Vec` deliberately retains declaration/output order; it is not a name-keyed
/// local type environment such as resolver [`RelationRow`](pure_analyzer_resolve::RelationRow).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelationSchema {
    columns: Vec<Column>,
}

impl RelationSchema {
    /// Construct an ordered schema, rejecting duplicate column identities.
    ///
    /// # Errors
    ///
    /// Returns [`SchemaError::DuplicateColumnId`] when two supplied columns use
    /// the same [`ColumnId`].
    pub fn new(columns: Vec<Column>) -> Result<Self, SchemaError> {
        let mut ids = BTreeSet::new();
        for column in &columns {
            if !ids.insert(column.id()) {
                return Err(SchemaError::DuplicateColumnId(column.id()));
            }
        }
        Ok(Self { columns })
    }

    /// Return columns in declaration/output order.
    #[must_use]
    pub fn columns(&self) -> &[Column] {
        &self.columns
    }

    /// Return a column by stable identity without changing schema order.
    #[must_use]
    pub fn column(&self, id: ColumnId) -> Option<&Column> {
        self.columns.iter().find(|column| column.id() == id)
    }
}

/// A candidate key identified by its constituent output columns.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct CandidateKey {
    columns: Vec<ColumnId>,
}

impl CandidateKey {
    /// Construct a canonical representation of one candidate-key set.
    ///
    /// Column identities are sorted and deduplicated because a key is a set.
    /// The empty key is valid and proves an at-most-one-row relation.
    #[must_use]
    pub fn new(mut columns: Vec<ColumnId>) -> Self {
        columns.sort_unstable();
        columns.dedup();
        Self { columns }
    }

    /// Return the key columns in their stable presentation order.
    #[must_use]
    pub fn columns(&self) -> &[ColumnId] {
        &self.columns
    }
}

/// Whether a scalar expression is proven defined for every input row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Totality {
    /// The expression is proven defined for every input row.
    Total,
    /// The expression is not defined for at least some input rows.
    Partial,
}

/// Whether a relation is governed by set or bag row semantics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowSemantics {
    /// Duplicate rows have no observable multiplicity.
    Set,
    /// Duplicate rows have observable multiplicity.
    Bag,
}

/// A fact that is either supported by explicit evidence or deliberately unknown.
///
/// This wrapper prevents downstream consumers from treating absent facts as a
/// default. In particular, association multiplicity alone never creates a
/// proven key, totality, or row-semantics fact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Knowledge<T> {
    /// A fact with its query and model evidence.
    Proven {
        /// The established fact.
        value: T,
        /// The query span and model facts that establish the value.
        origin: IrOrigin,
    },
    /// The available facts do not establish a safe conclusion.
    Unknown,
}

impl<T> Knowledge<T> {
    /// Construct a proven fact with explicit evidence.
    #[must_use]
    pub fn proven(value: T, origin: IrOrigin) -> Self {
        Self::Proven { value, origin }
    }

    /// Construct an explicitly unknown fact.
    #[must_use]
    pub const fn unknown() -> Self {
        Self::Unknown
    }

    /// Borrow the proven value and evidence, if the fact is established.
    #[must_use]
    pub fn as_proven(&self) -> Option<(&T, &IrOrigin)> {
        match self {
            Self::Proven { value, origin } => Some((value, origin)),
            Self::Unknown => None,
        }
    }

    /// Return whether no safe conclusion is available.
    #[must_use]
    pub const fn is_unknown(&self) -> bool {
        matches!(self, Self::Unknown)
    }
}

/// Relational facts that must be proved separately from value multiplicity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelationFacts {
    candidate_keys: Knowledge<Vec<CandidateKey>>,
    row_semantics: Knowledge<RowSemantics>,
}

impl RelationFacts {
    /// Construct relation facts with independently established evidence.
    #[must_use]
    pub fn new(
        candidate_keys: Knowledge<Vec<CandidateKey>>,
        row_semantics: Knowledge<RowSemantics>,
    ) -> Self {
        Self {
            candidate_keys: canonicalize_candidate_keys(candidate_keys),
            row_semantics,
        }
    }

    /// Construct the conservative state where no relational fact is assumed.
    #[must_use]
    pub fn unknown() -> Self {
        Self::new(Knowledge::Unknown, Knowledge::Unknown)
    }

    /// Return candidate-key knowledge for the relation output.
    #[must_use]
    pub const fn candidate_keys(&self) -> &Knowledge<Vec<CandidateKey>> {
        &self.candidate_keys
    }

    /// Return whether set or bag row semantics are proven.
    #[must_use]
    pub const fn row_semantics(&self) -> &Knowledge<RowSemantics> {
        &self.row_semantics
    }
}

/// A relational-expression construction invariant failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum RelationExpressionError {
    /// A filter did not retain its input schema.
    #[error("filter output schema differs from its input schema")]
    FilterSchemaMismatch,
    /// A projection did not bind every output column in schema order.
    #[error("projection bindings do not match the output schema")]
    ProjectionSchemaMismatch,
    /// A join output was not the ordered concatenation of its inputs.
    #[error("join output schema is not the ordered input concatenation")]
    JoinSchemaMismatch,
    /// A distinct expression did not retain its input schema.
    #[error("distinct output schema differs from its input schema")]
    DistinctSchemaMismatch,
    /// A scalar expression referred to a column outside its input scope.
    #[error("scalar expression references a column outside its input schema")]
    UnknownColumnReference(ColumnId),
    /// A proven candidate key references a column outside the output schema.
    #[error("proven candidate key references a column outside its output schema")]
    UnknownKeyColumn(ColumnId),
    /// A column reference did not retain the referenced column's type facts.
    #[error("column reference does not retain the referenced column metadata")]
    ColumnMetadataMismatch(ColumnId),
    /// A navigation receiver or result did not retain its resolved model facts.
    #[error("navigation does not retain resolved receiver or member metadata")]
    NavigationMetadataMismatch,
    /// Equality compared two scalar expressions with distinct exact types.
    #[error("equality operands do not have the same exact type")]
    ComparisonTypeMismatch,
    /// A supported literal did not have its required type, multiplicity, or nullability.
    #[error("literal metadata does not match its supported scalar form")]
    InvalidLiteralType,
    /// A filter, join, or Boolean operator did not have Boolean `[1]` non-null facts.
    #[error("predicate does not have Boolean [1] non-null metadata")]
    NonBooleanPredicate,
}

/// A resolved scan source in the supported relational core.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RelationSource {
    /// A resolved class source.
    Class(ResolvedClass),
    /// A resolved member source, retaining the exact association or property identity.
    Member(ResolvedMember),
}

/// Resolver-issued evidence for one supported correlated member navigation.
///
/// The private receiver preserves the exact local value from which the resolver
/// selected the member. It prevents callers from combining an arbitrary member
/// with an unrelated scalar expression in [`ScalarOperator::Navigation`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedNavigation {
    receiver: LocalValue,
    member: ResolvedMember,
}

impl ResolvedNavigation {
    /// Retain one resolver-proven, argument-free to-one member navigation.
    ///
    /// Returns `None` unless `chain` contains exactly one property or
    /// association-end hop from a class receiver, with no arguments and a
    /// to-one declared member multiplicity. [`NavigationChain`] values can
    /// only be created by [`pure_analyzer_resolve::NavigationResolver`].
    #[must_use]
    pub fn from_chain(chain: &NavigationChain) -> Option<Self> {
        let [hop] = chain.hops() else {
            return None;
        };
        let NavigationTarget::Member(member) = hop.target() else {
            return None;
        };
        // `NavigationChain` is only resolver-constructed. A member hop is
        // therefore already class-rooted and argument-valid; retain the two
        // restrictions that define this relational subset.
        if !matches!(
            member.kind(),
            ResolvedMemberKind::Property | ResolvedMemberKind::AssociationEnd { .. }
        ) || !member.multiplicity().is_to_one()
        {
            return None;
        }
        Some(Self {
            receiver: chain.source().clone(),
            member: member.clone(),
        })
    }

    /// Return the exact resolved member selected by the navigation proof.
    #[must_use]
    pub const fn member(&self) -> &ResolvedMember {
        &self.member
    }

    pub(crate) const fn receiver(&self) -> &LocalValue {
        &self.receiver
    }
}

/// One supported join form in the initial decidable relational core.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JoinKind {
    /// Rows are emitted only when the join condition holds.
    Inner,
}

/// A projected output-column expression.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Projection {
    column: ColumnId,
    expression: ScalarExpression,
}

impl Projection {
    /// Bind a scalar expression to one explicit output column identity.
    #[must_use]
    pub fn new(column: ColumnId, expression: ScalarExpression) -> Self {
        Self { column, expression }
    }

    /// Return the output column populated by this projection.
    #[must_use]
    pub const fn column(&self) -> ColumnId {
        self.column
    }

    /// Return the expression that populates the output column.
    #[must_use]
    pub const fn expression(&self) -> &ScalarExpression {
        &self.expression
    }
}

/// The closed supported set of relational operators.
///
/// Unsupported source constructs do not appear in this enum; lowering returns
/// [`RelationalOutcome::Opaque`] with a typed [`ReasonCode`] instead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RelationOperator {
    /// Read a resolved class or member source.
    Scan(RelationSource),
    /// Keep rows satisfying a supported scalar predicate.
    Filter {
        /// Input relation.
        input: Box<RelationExpression>,
        /// Predicate evaluated for each input row.
        predicate: ScalarExpression,
    },
    /// Produce ordered output columns from scalar expressions.
    Project {
        /// Input relation.
        input: Box<RelationExpression>,
        /// Output bindings in schema order.
        projections: Vec<Projection>,
    },
    /// Combine two inputs with a supported inner-join condition.
    Join {
        /// The supported join form.
        kind: JoinKind,
        /// Left input relation.
        left: Box<RelationExpression>,
        /// Right input relation.
        right: Box<RelationExpression>,
        /// Join condition evaluated over the combined row.
        condition: ScalarExpression,
    },
    /// Remove duplicate rows; lowering attaches explicit set-semantics evidence
    /// when it has a proven implementation of this operation.
    Distinct {
        /// Input relation.
        input: Box<RelationExpression>,
    },
}

/// A typed relational expression with an ordered schema and stable origin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelationExpression {
    operator: RelationOperator,
    schema: RelationSchema,
    facts: RelationFacts,
    origin: IrOrigin,
}

impl RelationExpression {
    /// Construct a relational expression after validating its typed shape.
    ///
    /// # Errors
    ///
    /// Returns [`RelationExpressionError`] when an operator does not preserve
    /// its required schema or a scalar refers outside its input scope.
    pub fn new(
        operator: RelationOperator,
        schema: RelationSchema,
        facts: RelationFacts,
        origin: IrOrigin,
    ) -> Result<Self, RelationExpressionError> {
        validate_relation_operator(&operator, &schema)?;
        validate_expression_keys(&schema, facts.candidate_keys())?;
        Ok(Self {
            operator,
            schema,
            facts,
            origin,
        })
    }

    /// Return the closed supported operator represented by this expression.
    #[must_use]
    pub const fn operator(&self) -> &RelationOperator {
        &self.operator
    }

    /// Return the explicitly ordered output schema.
    #[must_use]
    pub const fn schema(&self) -> &RelationSchema {
        &self.schema
    }

    /// Return the independently proven facts for this relation result.
    #[must_use]
    pub const fn facts(&self) -> &RelationFacts {
        &self.facts
    }

    /// Return query and model provenance for this expression.
    #[must_use]
    pub const fn origin(&self) -> &IrOrigin {
        &self.origin
    }
}

/// A literal admitted by the initial supported scalar core.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScalarLiteral {
    /// A Boolean literal.
    Boolean(bool),
    /// An integer literal.
    Integer(i64),
    /// A string literal.
    String(String),
    /// A `null` literal whose type is recorded by its enclosing expression.
    Null,
}

/// The closed supported set of scalar operators.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScalarOperator {
    /// Read one column by stable identity.
    Column(ColumnId),
    /// Embed a supported literal.
    Literal(ScalarLiteral),
    /// Navigate a resolved model member from one input scalar.
    ///
    /// Keeping `input` explicit preserves the correlation between a row
    /// element and the member value derived from it.
    Navigation {
        /// Input value that owns the resolved member navigation.
        input: Box<ScalarExpression>,
        /// Resolver-issued proof for the exact member and receiver.
        navigation: Box<ResolvedNavigation>,
    },
    /// Compare two scalar expressions for equality.
    Equal {
        /// Left operand.
        left: Box<ScalarExpression>,
        /// Right operand.
        right: Box<ScalarExpression>,
    },
    /// Require both Boolean operands to hold.
    And {
        /// Left operand.
        left: Box<ScalarExpression>,
        /// Right operand.
        right: Box<ScalarExpression>,
    },
    /// Require either Boolean operand to hold.
    Or {
        /// Left operand.
        left: Box<ScalarExpression>,
        /// Right operand.
        right: Box<ScalarExpression>,
    },
    /// Negate a Boolean operand.
    Not {
        /// Operand to negate.
        input: Box<ScalarExpression>,
    },
}

/// A typed scalar expression in a relational operator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScalarExpression {
    operator: ScalarOperator,
    type_ref: TypeRef,
    multiplicity: Multiplicity,
    nullability: Nullability,
    totality: Knowledge<Totality>,
    origin: IrOrigin,
}

impl ScalarExpression {
    /// Construct a scalar expression with all type and totality facts explicit.
    #[must_use]
    pub fn new(
        operator: ScalarOperator,
        type_ref: TypeRef,
        multiplicity: Multiplicity,
        nullability: Nullability,
        totality: Knowledge<Totality>,
        origin: IrOrigin,
    ) -> Self {
        Self {
            operator,
            type_ref,
            multiplicity,
            nullability,
            totality,
            origin,
        }
    }

    /// Return the closed supported scalar operator.
    #[must_use]
    pub const fn operator(&self) -> &ScalarOperator {
        &self.operator
    }

    /// Return the expression's explicit Pure type.
    #[must_use]
    pub const fn type_ref(&self) -> &TypeRef {
        &self.type_ref
    }

    /// Return the expression's value multiplicity.
    #[must_use]
    pub const fn multiplicity(&self) -> Multiplicity {
        self.multiplicity
    }

    /// Return explicit nullability knowledge.
    #[must_use]
    pub const fn nullability(&self) -> Nullability {
        self.nullability
    }

    /// Return whether this scalar expression is proven defined for every row.
    #[must_use]
    pub const fn totality(&self) -> &Knowledge<Totality> {
        &self.totality
    }

    /// Return query and model provenance for this expression.
    #[must_use]
    pub const fn origin(&self) -> &IrOrigin {
        &self.origin
    }
}

/// A deliberately opaque result for syntax outside the supported core.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpaqueOutcome {
    reason: ReasonCode,
    origin: IrOrigin,
}

impl OpaqueOutcome {
    /// Construct an opaque outcome with a registry-backed reason and origin.
    #[must_use]
    pub fn new(reason: ReasonCode, origin: IrOrigin) -> Self {
        Self { reason, origin }
    }

    /// Return the stable reason code for declining a supported-core result.
    #[must_use]
    pub const fn reason(&self) -> ReasonCode {
        self.reason
    }

    /// Return the query and model provenance of the unsupported construct.
    #[must_use]
    pub const fn origin(&self) -> &IrOrigin {
        &self.origin
    }
}

/// The conservative outcome of relational lowering.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RelationalOutcome {
    /// A query represented entirely by the closed supported core.
    Supported(Box<RelationalQuery>),
    /// An unsupported construct or unavailable fact with an explicit reason.
    Opaque(OpaqueOutcome),
}

impl RelationalOutcome {
    /// Wrap a supported query.
    #[must_use]
    pub fn supported(query: RelationalQuery) -> Self {
        Self::Supported(Box::new(query))
    }

    /// Wrap an explicit opaque result.
    #[must_use]
    pub fn opaque(outcome: OpaqueOutcome) -> Self {
        Self::Opaque(outcome)
    }
}

/// A complete supported relational query and its separately proven facts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelationalQuery {
    root: RelationExpression,
}

impl RelationalQuery {
    /// Construct a supported query from its already-validated root expression.
    #[must_use]
    pub fn new(root: RelationExpression) -> Self {
        Self { root }
    }

    /// Return the root relational expression.
    #[must_use]
    pub const fn root(&self) -> &RelationExpression {
        &self.root
    }

    /// Return the ordered output schema.
    #[must_use]
    pub const fn output(&self) -> &RelationSchema {
        self.root.schema()
    }

    /// Return separately proven relation facts.
    #[must_use]
    pub const fn facts(&self) -> &RelationFacts {
        self.root.facts()
    }
}

fn validate_relation_operator(
    operator: &RelationOperator,
    schema: &RelationSchema,
) -> Result<(), RelationExpressionError> {
    match operator {
        RelationOperator::Scan(_) => Ok(()),
        RelationOperator::Filter { input, predicate } => {
            if schema != input.schema() {
                return Err(RelationExpressionError::FilterSchemaMismatch);
            }
            validate_scalar(predicate, &[input.schema()])?;
            validate_boolean(predicate)
        }
        RelationOperator::Project { input, projections } => {
            validate_projection_schema(projections, schema)?;
            for (projection, column) in projections.iter().zip(schema.columns()) {
                validate_scalar(projection.expression(), &[input.schema()])?;
                if !same_column_metadata(projection.expression(), column) {
                    return Err(RelationExpressionError::ColumnMetadataMismatch(
                        projection.column(),
                    ));
                }
            }
            Ok(())
        }
        RelationOperator::Join {
            left,
            right,
            condition,
            ..
        } => {
            if !is_join_schema(schema, left.schema(), right.schema()) {
                return Err(RelationExpressionError::JoinSchemaMismatch);
            }
            let input_schemas = [left.schema(), right.schema()];
            validate_scalar(condition, &input_schemas)?;
            validate_boolean(condition)
        }
        RelationOperator::Distinct { input } => {
            if schema != input.schema() {
                return Err(RelationExpressionError::DistinctSchemaMismatch);
            }
            Ok(())
        }
    }
}

fn validate_projection_schema(
    projections: &[Projection],
    schema: &RelationSchema,
) -> Result<(), RelationExpressionError> {
    if projections.len() != schema.columns().len()
        || projections
            .iter()
            .zip(schema.columns())
            .any(|(projection, column)| projection.column() != column.id())
    {
        return Err(RelationExpressionError::ProjectionSchemaMismatch);
    }
    Ok(())
}

fn is_join_schema(schema: &RelationSchema, left: &RelationSchema, right: &RelationSchema) -> bool {
    schema
        .columns()
        .iter()
        .eq(left.columns().iter().chain(right.columns()))
}

fn validate_scalar(
    expression: &ScalarExpression,
    input_schemas: &[&RelationSchema],
) -> Result<(), RelationExpressionError> {
    match expression.operator() {
        ScalarOperator::Column(id) => {
            let Some(column) = input_schemas.iter().find_map(|schema| schema.column(*id)) else {
                return Err(RelationExpressionError::UnknownColumnReference(*id));
            };
            if !same_column_metadata(expression, column) {
                return Err(RelationExpressionError::ColumnMetadataMismatch(*id));
            }
            Ok(())
        }
        ScalarOperator::Literal(literal) => validate_literal(expression, literal),
        ScalarOperator::Navigation { input, navigation } => {
            validate_scalar(input, input_schemas)?;
            if !navigation_receiver_matches(input, navigation.receiver()) {
                return Err(RelationExpressionError::NavigationMetadataMismatch);
            }
            let member = navigation.member();
            let Some(expected_multiplicity) =
                compose_navigation_multiplicity(input.multiplicity(), member.multiplicity())
            else {
                return Err(RelationExpressionError::NavigationMetadataMismatch);
            };
            if expression.type_ref() != member.target()
                || expression.multiplicity() != expected_multiplicity
                || expression.nullability() != Nullability::Unknown
                || !expression
                    .origin()
                    .model_origins()
                    .contains(&ModelOrigin::from_member(member))
            {
                return Err(RelationExpressionError::NavigationMetadataMismatch);
            }
            Ok(())
        }
        ScalarOperator::Equal { left, right } => {
            validate_scalar(left, input_schemas)?;
            validate_scalar(right, input_schemas)?;
            if left.type_ref() != right.type_ref() {
                return Err(RelationExpressionError::ComparisonTypeMismatch);
            }
            validate_boolean(expression)
        }
        ScalarOperator::And { left, right } | ScalarOperator::Or { left, right } => {
            validate_scalar(left, input_schemas)?;
            validate_scalar(right, input_schemas)?;
            validate_boolean(left)?;
            validate_boolean(right)?;
            validate_boolean(expression)
        }
        ScalarOperator::Not { input } => {
            validate_scalar(input, input_schemas)?;
            validate_boolean(input)?;
            validate_boolean(expression)
        }
    }
}

fn same_column_metadata(expression: &ScalarExpression, column: &Column) -> bool {
    expression.type_ref() == column.type_ref()
        && expression.multiplicity() == column.multiplicity()
        && expression.nullability() == column.nullability()
}

fn navigation_receiver_matches(expression: &ScalarExpression, receiver: &LocalValue) -> bool {
    let LocalValueKind::Class(class) = receiver.kind() else {
        return false;
    };
    expression.type_ref() == &TypeRef::new(class.path().clone(), Vec::new())
        && expression.multiplicity() == receiver.multiplicity()
        && expression
            .origin()
            .model_origins()
            .contains(&ModelOrigin::from_class(class))
}

fn validate_literal(
    expression: &ScalarExpression,
    literal: &ScalarLiteral,
) -> Result<(), RelationExpressionError> {
    let one_value = is_exactly_one(expression.multiplicity());
    let primitive_type = expression.type_ref().type_arguments().is_empty();
    let type_name = expression.type_ref().raw_type().as_str();
    let valid = match literal {
        ScalarLiteral::Boolean(_) => {
            primitive_type
                && type_name == BOOLEAN_TYPE
                && one_value
                && expression.nullability() == Nullability::NonNullable
        }
        ScalarLiteral::Integer(_) => {
            primitive_type
                && type_name == INTEGER_TYPE
                && one_value
                && expression.nullability() == Nullability::NonNullable
        }
        ScalarLiteral::String(_) => {
            primitive_type
                && type_name == STRING_TYPE
                && one_value
                && expression.nullability() == Nullability::NonNullable
        }
        ScalarLiteral::Null => one_value && expression.nullability() == Nullability::Nullable,
    };
    if valid {
        Ok(())
    } else {
        Err(RelationExpressionError::InvalidLiteralType)
    }
}

fn validate_boolean(expression: &ScalarExpression) -> Result<(), RelationExpressionError> {
    if expression.type_ref().raw_type().as_str() == BOOLEAN_TYPE
        && expression.type_ref().type_arguments().is_empty()
        && is_exactly_one(expression.multiplicity())
        && expression.nullability() == Nullability::NonNullable
    {
        Ok(())
    } else {
        Err(RelationExpressionError::NonBooleanPredicate)
    }
}

fn is_exactly_one(multiplicity: Multiplicity) -> bool {
    multiplicity.lower() == EXACTLY_ONE && multiplicity.upper() == Some(EXACTLY_ONE)
}

/// Compose receiver and member cardinalities for correlated navigation.
///
/// The result is absent only when multiplying two finite upper bounds would
/// overflow the model's representable multiplicity range.
pub(crate) fn compose_navigation_multiplicity(
    receiver: Multiplicity,
    member: Multiplicity,
) -> Option<Multiplicity> {
    let lower = receiver.lower().checked_mul(member.lower())?;
    let upper = match (receiver.upper(), member.upper()) {
        (Some(0), _) | (_, Some(0)) => Some(0),
        (Some(left), Some(right)) => Some(left.checked_mul(right)?),
        _ => None,
    };
    Multiplicity::new(lower, upper).ok()
}

fn validate_expression_keys(
    schema: &RelationSchema,
    candidate_keys: &Knowledge<Vec<CandidateKey>>,
) -> Result<(), RelationExpressionError> {
    let Some((keys, _)) = candidate_keys.as_proven() else {
        return Ok(());
    };

    for key in keys {
        for column in key.columns() {
            if schema.column(*column).is_none() {
                return Err(RelationExpressionError::UnknownKeyColumn(*column));
            }
        }
    }
    Ok(())
}

fn canonicalize_candidate_keys(
    candidate_keys: Knowledge<Vec<CandidateKey>>,
) -> Knowledge<Vec<CandidateKey>> {
    match candidate_keys {
        Knowledge::Proven { mut value, origin } => {
            value.sort_unstable();
            value.dedup();
            Knowledge::Proven { value, origin }
        }
        Knowledge::Unknown => Knowledge::Unknown,
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests {
    use pure_analyzer_diagnostics::{TextRange, TextSize};
    use pure_analyzer_model::{PmcdDocument, QName, load_pmcd_documents};
    use pure_analyzer_resolve::{
        LocalValue, NavigationChain, NavigationResolution, NavigationResolver, NavigationStep,
        Resolution, Resolver,
    };
    use serde_json::json;

    use super::*;

    const QUERY_FILE: u32 = 17;
    const QUERY_START: u32 = 2;
    const QUERY_END: u32 = 13;
    const FIRST_COLUMN: u32 = 3;
    const SECOND_COLUMN: u32 = 11;
    const UNKNOWN_COLUMN: u32 = 23;
    const EXACTLY_ONE: u32 = 1;

    fn origin() -> IrOrigin {
        IrOrigin::new(
            SourceSpan::new(
                FileId::new(QUERY_FILE),
                TextRange::new(TextSize::from(QUERY_START), TextSize::from(QUERY_END)),
            ),
            Vec::new(),
        )
    }

    fn string_type() -> TypeRef {
        TypeRef::new(
            QName::new("String").expect("fixture type must be valid"),
            Vec::new(),
        )
    }

    fn boolean_type() -> TypeRef {
        TypeRef::new(
            QName::new(BOOLEAN_TYPE).expect("fixture type must be valid"),
            Vec::new(),
        )
    }

    fn integer_type() -> TypeRef {
        TypeRef::new(
            QName::new(INTEGER_TYPE).expect("fixture type must be valid"),
            Vec::new(),
        )
    }

    fn column(id: u32, name: &str, multiplicity: Multiplicity) -> Column {
        Column::new(
            ColumnId::new(id),
            Name::new(name).expect("fixture name must be valid"),
            string_type(),
            multiplicity,
            Nullability::Unknown,
            origin(),
        )
    }

    fn schema() -> RelationSchema {
        RelationSchema::new(vec![
            column(
                FIRST_COLUMN,
                "zeta",
                Multiplicity::new(EXACTLY_ONE, Some(EXACTLY_ONE))
                    .expect("fixture multiplicity must be valid"),
            ),
            column(SECOND_COLUMN, "alpha", Multiplicity::zero_or_more()),
        ])
        .expect("fixture schema must be valid")
    }

    fn resolved_class() -> ResolvedClass {
        let document = json!({
            "_type": "data",
            "elements": [{
                "_type": "class",
                "package": "model",
                "name": "Person",
                "stereotypes": [],
                "superTypes": [],
                "properties": [],
                "qualifiedProperties": []
            }]
        })
        .to_string();
        let graph = load_pmcd_documents(&[PmcdDocument::new("fixture", &document)])
            .expect("fixture model must load");
        let person = QName::new("model::Person").expect("fixture path must be valid");
        match Resolver::new(&graph).resolve_class(&person) {
            Resolution::Found(class) => class,
            outcome => panic!("fixture class must resolve, got {outcome:?}"),
        }
    }

    fn navigation_fixture() -> (pure_analyzer_model::ModelGraph, ResolvedClass) {
        let document = json!({
            "_type": "data",
            "elements": [
                {
                    "_type": "class",
                    "package": "model",
                    "name": "Person",
                    "stereotypes": [],
                    "superTypes": [],
                    "properties": [{
                        "name": "manager",
                        "genericType": {"rawType": "model::Manager", "typeArguments": []},
                        "multiplicity": {"lowerBound": 1, "upperBound": 1}
                    }, {
                        "name": "reports",
                        "genericType": {"rawType": "model::Manager", "typeArguments": []},
                        "multiplicity": {"lowerBound": 0, "upperBound": null}
                    }],
                    "qualifiedProperties": [{
                        "name": "zero",
                        "returnGenericType": {"rawType": "String", "typeArguments": []},
                        "returnMultiplicity": {"lowerBound": 1, "upperBound": 1},
                        "stereotypes": [],
                        "parameters": []
                    }]
                },
                {
                    "_type": "class",
                    "package": "model",
                    "name": "Manager",
                    "stereotypes": [],
                    "superTypes": [],
                    "properties": [],
                    "qualifiedProperties": []
                }
            ]
        })
        .to_string();
        let graph = load_pmcd_documents(&[PmcdDocument::new("fixture", &document)])
            .expect("fixture model must load");
        let person = QName::new("model::Person").expect("fixture path must be valid");
        let class = match Resolver::new(&graph).resolve_class(&person) {
            Resolution::Found(class) => class,
            outcome => panic!("fixture class must resolve, got {outcome:?}"),
        };
        (graph, class)
    }

    fn navigation_chain(
        graph: &pure_analyzer_model::ModelGraph,
        class: &ResolvedClass,
        name: &str,
    ) -> NavigationChain {
        let multiplicity = Multiplicity::new(EXACTLY_ONE, Some(EXACTLY_ONE))
            .expect("fixture multiplicity must be valid");
        let source = LocalValue::class(class.clone(), multiplicity);
        let step = NavigationStep::property(Name::new(name).expect("fixture name must be valid"));
        match NavigationResolver::new(graph).resolve(&source, &[step]) {
            NavigationResolution::Found(chain) => chain,
            outcome => panic!("fixture navigation must resolve, got {outcome:?}"),
        }
    }

    fn scan(schema: RelationSchema) -> RelationExpression {
        RelationExpression::new(
            RelationOperator::Scan(RelationSource::Class(resolved_class())),
            schema,
            RelationFacts::unknown(),
            origin(),
        )
        .expect("fixture scan must be valid")
    }

    fn scalar_column(column: &Column) -> ScalarExpression {
        ScalarExpression::new(
            ScalarOperator::Column(column.id()),
            column.type_ref().clone(),
            column.multiplicity(),
            column.nullability(),
            Knowledge::unknown(),
            origin(),
        )
    }

    fn boolean_literal(value: bool) -> ScalarExpression {
        ScalarExpression::new(
            ScalarOperator::Literal(ScalarLiteral::Boolean(value)),
            boolean_type(),
            Multiplicity::new(EXACTLY_ONE, Some(EXACTLY_ONE))
                .expect("fixture multiplicity must be valid"),
            Nullability::NonNullable,
            Knowledge::unknown(),
            origin(),
        )
    }

    fn integer_literal(value: i64) -> ScalarExpression {
        ScalarExpression::new(
            ScalarOperator::Literal(ScalarLiteral::Integer(value)),
            integer_type(),
            Multiplicity::new(EXACTLY_ONE, Some(EXACTLY_ONE))
                .expect("fixture multiplicity must be valid"),
            Nullability::NonNullable,
            Knowledge::unknown(),
            origin(),
        )
    }

    fn multiplicity(lower: u32, upper: Option<u32>) -> Multiplicity {
        Multiplicity::new(lower, upper).expect("fixture multiplicity must be valid")
    }

    fn type_with_argument(raw_type: &str) -> TypeRef {
        TypeRef::new(
            QName::new(raw_type).expect("fixture type must be valid"),
            vec![string_type()],
        )
    }

    fn literal_expression(
        literal: ScalarLiteral,
        type_ref: TypeRef,
        multiplicity: Multiplicity,
        nullability: Nullability,
    ) -> ScalarExpression {
        ScalarExpression::new(
            ScalarOperator::Literal(literal),
            type_ref,
            multiplicity,
            nullability,
            Knowledge::unknown(),
            origin(),
        )
    }

    fn assert_accepted_literal(
        literal: ScalarLiteral,
        type_ref: TypeRef,
        multiplicity: Multiplicity,
        nullability: Nullability,
    ) {
        let expression = literal_expression(literal.clone(), type_ref, multiplicity, nullability);

        assert_eq!(validate_literal(&expression, &literal), Ok(()));
    }

    fn assert_rejected_literal(
        literal: ScalarLiteral,
        type_ref: TypeRef,
        multiplicity: Multiplicity,
        nullability: Nullability,
    ) {
        let expression = literal_expression(literal.clone(), type_ref, multiplicity, nullability);

        assert_eq!(
            validate_literal(&expression, &literal),
            Err(RelationExpressionError::InvalidLiteralType)
        );
    }

    #[test]
    fn schema_preserves_explicit_output_order_and_identity() {
        let schema = schema();

        assert_eq!(schema.columns()[0].id(), ColumnId::new(FIRST_COLUMN));
        assert_eq!(schema.columns()[0].id().index(), FIRST_COLUMN);
        assert_eq!(schema.columns()[0].name().as_str(), "zeta");
        assert_eq!(schema.columns()[1].id(), ColumnId::new(SECOND_COLUMN));
        assert_eq!(schema.columns()[1].id().index(), SECOND_COLUMN);
        assert_eq!(schema.columns()[1].name().as_str(), "alpha");
    }

    #[test]
    fn schema_keeps_same_named_columns_distinct_by_identity() {
        let schema = RelationSchema::new(vec![
            column(FIRST_COLUMN, "value", Multiplicity::zero_or_more()),
            column(SECOND_COLUMN, "value", Multiplicity::zero_or_more()),
        ])
        .expect("fixture schema must be valid");

        assert_eq!(schema.columns()[0].name(), schema.columns()[1].name());
        assert_ne!(schema.columns()[0].id(), schema.columns()[1].id());
    }

    #[test]
    fn schema_rejects_duplicate_column_identity_without_reordering_columns() {
        let duplicate = ColumnId::new(FIRST_COLUMN);
        let result = RelationSchema::new(vec![
            column(
                FIRST_COLUMN,
                "first",
                Multiplicity::new(EXACTLY_ONE, Some(EXACTLY_ONE))
                    .expect("fixture multiplicity must be valid"),
            ),
            column(FIRST_COLUMN, "second", Multiplicity::zero_or_more()),
        ]);

        assert_eq!(result, Err(SchemaError::DuplicateColumnId(duplicate)));
    }

    #[test]
    fn query_and_model_origins_remain_distinct() {
        let class = resolved_class();
        let model_origin = ModelOrigin::from_class(&class);
        let query_origin = IrOrigin::new(origin().source(), vec![model_origin.clone()]);

        assert_eq!(query_origin.source().file(), FileId::new(QUERY_FILE));
        assert_eq!(
            query_origin.model_origins(),
            std::slice::from_ref(&model_origin)
        );
        assert_ne!(
            query_origin.source().file(),
            model_origin.definition().source().file_id()
        );
    }

    #[test]
    fn relation_facts_stay_unknown_for_to_one_value_multiplicity() {
        let relation_schema = schema();
        let facts = RelationFacts::unknown();
        let bag_facts = RelationFacts::new(
            Knowledge::unknown(),
            Knowledge::proven(RowSemantics::Bag, origin()),
        );
        let set_facts = RelationFacts::new(
            Knowledge::unknown(),
            Knowledge::proven(RowSemantics::Set, origin()),
        );
        let scalar = ScalarExpression::new(
            ScalarOperator::Column(ColumnId::new(FIRST_COLUMN)),
            string_type(),
            Multiplicity::new(EXACTLY_ONE, Some(EXACTLY_ONE))
                .expect("fixture multiplicity must be valid"),
            Nullability::Unknown,
            Knowledge::unknown(),
            origin(),
        );

        assert_eq!(
            relation_schema.columns()[0].multiplicity(),
            Multiplicity::new(EXACTLY_ONE, Some(EXACTLY_ONE))
                .expect("fixture multiplicity must be valid")
        );
        assert!(facts.candidate_keys().is_unknown());
        assert!(facts.row_semantics().is_unknown());
        assert!(scalar.totality().is_unknown());
        assert!(!Knowledge::proven(RowSemantics::Bag, origin()).is_unknown());
        assert!(matches!(
            bag_facts.row_semantics(),
            Knowledge::Proven {
                value: RowSemantics::Bag,
                ..
            }
        ));
        assert!(matches!(
            set_facts.row_semantics(),
            Knowledge::Proven {
                value: RowSemantics::Set,
                ..
            }
        ));
    }

    #[test]
    fn candidate_key_and_key_facts_are_canonical_and_unknown_is_distinct() {
        let first = ColumnId::new(FIRST_COLUMN);
        let second = ColumnId::new(SECOND_COLUMN);
        let key = CandidateKey::new(vec![second, first, second]);
        let facts = RelationFacts::new(
            Knowledge::proven(vec![key.clone(), key.clone()], origin()),
            Knowledge::unknown(),
        );
        let known_empty = RelationFacts::new(
            Knowledge::proven(Vec::<CandidateKey>::new(), origin()),
            Knowledge::unknown(),
        );

        assert_eq!(key.columns(), &[first, second]);
        assert!(matches!(
            facts.candidate_keys().as_proven(),
            Some((keys, _)) if keys == &[key]
        ));
        assert!(matches!(
            known_empty.candidate_keys().as_proven(),
            Some((keys, _)) if keys.is_empty()
        ));
        assert!(RelationFacts::unknown().candidate_keys().is_unknown());
    }

    #[test]
    fn scalar_validation_preserves_every_column_metadata_field() {
        let column = column(
            FIRST_COLUMN,
            "value",
            multiplicity(EXACTLY_ONE, Some(EXACTLY_ONE)),
        );
        let matching = scalar_column(&column);
        let wrong_type = ScalarExpression::new(
            ScalarOperator::Column(column.id()),
            boolean_type(),
            column.multiplicity(),
            column.nullability(),
            Knowledge::unknown(),
            origin(),
        );
        let wrong_multiplicity = ScalarExpression::new(
            ScalarOperator::Column(column.id()),
            column.type_ref().clone(),
            Multiplicity::zero_or_more(),
            column.nullability(),
            Knowledge::unknown(),
            origin(),
        );
        let wrong_nullability = ScalarExpression::new(
            ScalarOperator::Column(column.id()),
            column.type_ref().clone(),
            column.multiplicity(),
            Nullability::NonNullable,
            Knowledge::unknown(),
            origin(),
        );

        assert!(same_column_metadata(&matching, &column));
        assert!(!same_column_metadata(&wrong_type, &column));
        assert!(!same_column_metadata(&wrong_multiplicity, &column));
        assert!(!same_column_metadata(&wrong_nullability, &column));
    }

    #[test]
    fn join_schema_requires_the_ordered_concatenation_of_both_inputs() {
        let left = schema();
        let right = RelationSchema::new(vec![column(
            UNKNOWN_COLUMN,
            "right",
            Multiplicity::zero_or_more(),
        )])
        .expect("fixture schema must be valid");
        let joined = RelationSchema::new(
            left.columns()
                .iter()
                .chain(right.columns())
                .cloned()
                .collect(),
        )
        .expect("fixture schema must be valid");

        assert!(is_join_schema(&joined, &left, &right));
        assert!(!is_join_schema(&left, &left, &right));
    }

    #[test]
    fn literal_validation_requires_every_supported_metadata_fact() {
        let exactly_one = multiplicity(EXACTLY_ONE, Some(EXACTLY_ONE));
        let optional = multiplicity(0, Some(EXACTLY_ONE));

        assert_accepted_literal(
            ScalarLiteral::Boolean(true),
            boolean_type(),
            exactly_one,
            Nullability::NonNullable,
        );
        assert_rejected_literal(
            ScalarLiteral::Boolean(true),
            type_with_argument(BOOLEAN_TYPE),
            exactly_one,
            Nullability::NonNullable,
        );
        assert_rejected_literal(
            ScalarLiteral::Boolean(true),
            string_type(),
            exactly_one,
            Nullability::NonNullable,
        );
        assert_rejected_literal(
            ScalarLiteral::Boolean(true),
            boolean_type(),
            optional,
            Nullability::NonNullable,
        );
        assert_rejected_literal(
            ScalarLiteral::Boolean(true),
            boolean_type(),
            exactly_one,
            Nullability::Nullable,
        );

        assert_accepted_literal(
            ScalarLiteral::Integer(7),
            integer_type(),
            exactly_one,
            Nullability::NonNullable,
        );
        assert_rejected_literal(
            ScalarLiteral::Integer(7),
            type_with_argument(INTEGER_TYPE),
            exactly_one,
            Nullability::NonNullable,
        );
        assert_rejected_literal(
            ScalarLiteral::Integer(7),
            string_type(),
            exactly_one,
            Nullability::NonNullable,
        );
        assert_rejected_literal(
            ScalarLiteral::Integer(7),
            integer_type(),
            optional,
            Nullability::NonNullable,
        );
        assert_rejected_literal(
            ScalarLiteral::Integer(7),
            integer_type(),
            exactly_one,
            Nullability::Nullable,
        );

        assert_accepted_literal(
            ScalarLiteral::String("value".to_owned()),
            string_type(),
            exactly_one,
            Nullability::NonNullable,
        );
        assert_rejected_literal(
            ScalarLiteral::String("value".to_owned()),
            type_with_argument(STRING_TYPE),
            exactly_one,
            Nullability::NonNullable,
        );
        assert_rejected_literal(
            ScalarLiteral::String("value".to_owned()),
            boolean_type(),
            exactly_one,
            Nullability::NonNullable,
        );
        assert_rejected_literal(
            ScalarLiteral::String("value".to_owned()),
            string_type(),
            optional,
            Nullability::NonNullable,
        );
        assert_rejected_literal(
            ScalarLiteral::String("value".to_owned()),
            string_type(),
            exactly_one,
            Nullability::Nullable,
        );

        assert_accepted_literal(
            ScalarLiteral::Null,
            boolean_type(),
            exactly_one,
            Nullability::Nullable,
        );
        assert_rejected_literal(
            ScalarLiteral::Null,
            boolean_type(),
            optional,
            Nullability::Nullable,
        );
        assert_rejected_literal(
            ScalarLiteral::Null,
            boolean_type(),
            exactly_one,
            Nullability::NonNullable,
        );
    }

    #[test]
    fn boolean_validation_requires_boolean_exactly_one_non_null_metadata() {
        let valid = boolean_literal(true);
        let wrong_type = ScalarExpression::new(
            ScalarOperator::Literal(ScalarLiteral::Boolean(true)),
            integer_type(),
            multiplicity(EXACTLY_ONE, Some(EXACTLY_ONE)),
            Nullability::NonNullable,
            Knowledge::unknown(),
            origin(),
        );
        let nullable = ScalarExpression::new(
            ScalarOperator::Literal(ScalarLiteral::Boolean(true)),
            boolean_type(),
            multiplicity(EXACTLY_ONE, Some(EXACTLY_ONE)),
            Nullability::Nullable,
            Knowledge::unknown(),
            origin(),
        );

        assert_eq!(validate_boolean(&valid), Ok(()));
        assert_eq!(
            validate_boolean(&wrong_type),
            Err(RelationExpressionError::NonBooleanPredicate)
        );
        assert_eq!(
            validate_boolean(&nullable),
            Err(RelationExpressionError::NonBooleanPredicate)
        );
        assert!(is_exactly_one(multiplicity(EXACTLY_ONE, Some(EXACTLY_ONE))));
        assert!(!is_exactly_one(multiplicity(0, Some(EXACTLY_ONE))));
        assert!(!is_exactly_one(multiplicity(EXACTLY_ONE, None)));
    }

    #[test]
    fn relation_expression_rejects_proven_keys_outside_its_output_schema() {
        let unknown_key_column = ColumnId::new(UNKNOWN_COLUMN);
        let key = CandidateKey::new(vec![unknown_key_column]);
        let facts =
            RelationFacts::new(Knowledge::proven(vec![key], origin()), Knowledge::unknown());

        let result = RelationExpression::new(
            RelationOperator::Scan(RelationSource::Class(resolved_class())),
            schema(),
            facts,
            origin(),
        );

        assert_eq!(
            result,
            Err(RelationExpressionError::UnknownKeyColumn(
                unknown_key_column
            ))
        );
    }

    #[test]
    fn projection_must_bind_each_output_column_once_in_schema_order() {
        let output = schema();
        let input = scan(output.clone());
        let valid = RelationExpression::new(
            RelationOperator::Project {
                input: Box::new(input),
                projections: output
                    .columns()
                    .iter()
                    .map(|column| Projection::new(column.id(), scalar_column(column)))
                    .collect(),
            },
            output.clone(),
            RelationFacts::unknown(),
            origin(),
        );
        let incomplete = RelationExpression::new(
            RelationOperator::Project {
                input: Box::new(scan(output.clone())),
                projections: vec![Projection::new(
                    output.columns()[0].id(),
                    scalar_column(&output.columns()[0]),
                )],
            },
            output,
            RelationFacts::unknown(),
            origin(),
        );

        assert!(valid.is_ok());
        assert_eq!(
            incomplete,
            Err(RelationExpressionError::ProjectionSchemaMismatch)
        );
    }

    #[test]
    fn scalar_references_and_predicates_are_checked_against_input_schema() {
        let input = scan(schema());
        let output = input.schema().clone();
        let unknown_column = ScalarExpression::new(
            ScalarOperator::Column(ColumnId::new(UNKNOWN_COLUMN)),
            boolean_type(),
            Multiplicity::zero_or_more(),
            Nullability::Unknown,
            Knowledge::unknown(),
            origin(),
        );
        let unknown_result = RelationExpression::new(
            RelationOperator::Filter {
                input: Box::new(input),
                predicate: unknown_column,
            },
            output,
            RelationFacts::unknown(),
            origin(),
        );

        let input = scan(schema());
        let output = input.schema().clone();
        let non_boolean_result = RelationExpression::new(
            RelationOperator::Filter {
                predicate: scalar_column(&input.schema().columns()[0]),
                input: Box::new(input),
            },
            output,
            RelationFacts::unknown(),
            origin(),
        );

        assert_eq!(
            unknown_result,
            Err(RelationExpressionError::UnknownColumnReference(
                ColumnId::new(UNKNOWN_COLUMN)
            ))
        );
        assert_eq!(
            non_boolean_result,
            Err(RelationExpressionError::NonBooleanPredicate)
        );
    }

    #[test]
    fn scalar_core_rejects_mismatched_comparisons_and_malformed_literals() {
        let input = scan(schema());
        let output = input.schema().clone();
        let comparison = ScalarExpression::new(
            ScalarOperator::Equal {
                left: Box::new(scalar_column(&input.schema().columns()[0])),
                right: Box::new(integer_literal(7)),
            },
            boolean_type(),
            Multiplicity::new(EXACTLY_ONE, Some(EXACTLY_ONE))
                .expect("fixture multiplicity must be valid"),
            Nullability::NonNullable,
            Knowledge::unknown(),
            origin(),
        );
        let comparison_result = RelationExpression::new(
            RelationOperator::Filter {
                input: Box::new(input),
                predicate: comparison,
            },
            output,
            RelationFacts::unknown(),
            origin(),
        );

        let input = scan(schema());
        let output = input.schema().clone();
        let malformed_literal = ScalarExpression::new(
            ScalarOperator::Literal(ScalarLiteral::Boolean(true)),
            boolean_type(),
            Multiplicity::zero_or_more(),
            Nullability::NonNullable,
            Knowledge::unknown(),
            origin(),
        );
        let literal_result = RelationExpression::new(
            RelationOperator::Filter {
                input: Box::new(input),
                predicate: malformed_literal,
            },
            output,
            RelationFacts::unknown(),
            origin(),
        );

        let predicate_schema = RelationSchema::new(vec![Column::new(
            ColumnId::new(FIRST_COLUMN),
            Name::new("predicate").expect("fixture name must be valid"),
            boolean_type(),
            Multiplicity::zero_or_more(),
            Nullability::Nullable,
            origin(),
        )])
        .expect("fixture schema must be valid");
        let input = scan(predicate_schema);
        let output = input.schema().clone();
        let predicate_result = RelationExpression::new(
            RelationOperator::Filter {
                predicate: scalar_column(&input.schema().columns()[0]),
                input: Box::new(input),
            },
            output,
            RelationFacts::unknown(),
            origin(),
        );

        assert_eq!(
            comparison_result,
            Err(RelationExpressionError::ComparisonTypeMismatch)
        );
        assert_eq!(
            literal_result,
            Err(RelationExpressionError::InvalidLiteralType)
        );
        assert_eq!(
            predicate_result,
            Err(RelationExpressionError::NonBooleanPredicate)
        );
    }

    #[test]
    fn join_and_distinct_must_retain_their_defined_output_schemas() {
        let distinct_input = scan(schema());
        let distinct_result = RelationExpression::new(
            RelationOperator::Distinct {
                input: Box::new(distinct_input),
            },
            RelationSchema::new(vec![column(
                FIRST_COLUMN,
                "zeta",
                Multiplicity::zero_or_more(),
            )])
            .expect("fixture schema must be valid"),
            RelationFacts::unknown(),
            origin(),
        );

        let left_schema = schema();
        let right_schema = RelationSchema::new(vec![column(
            UNKNOWN_COLUMN,
            "right",
            Multiplicity::zero_or_more(),
        )])
        .expect("fixture schema must be valid");
        let join_result = RelationExpression::new(
            RelationOperator::Join {
                kind: JoinKind::Inner,
                left: Box::new(scan(left_schema.clone())),
                right: Box::new(scan(right_schema)),
                condition: boolean_literal(true),
            },
            left_schema,
            RelationFacts::unknown(),
            origin(),
        );

        assert_eq!(
            distinct_result,
            Err(RelationExpressionError::DistinctSchemaMismatch)
        );
        assert_eq!(
            join_result,
            Err(RelationExpressionError::JoinSchemaMismatch)
        );
    }

    #[test]
    fn opaque_outcomes_carry_a_registry_reason_and_stable_origin() {
        let opaque = OpaqueOutcome::new(ReasonCode::IndUnmodeledOp, origin());
        let outcome = RelationalOutcome::opaque(opaque.clone());

        assert!(matches!(outcome, RelationalOutcome::Opaque(value) if value == opaque));
        assert_eq!(opaque.reason(), ReasonCode::IndUnmodeledOp);
        assert_eq!(
            opaque.origin().source().range().start(),
            TextSize::from(QUERY_START)
        );
    }

    #[test]
    fn supported_query_retains_output_schema_and_explicit_unknown_facts() {
        let query = RelationalQuery::new(scan(schema()));
        let outcome = RelationalOutcome::supported(query.clone());

        assert_eq!(query.output().columns().len(), 2);
        assert!(query.facts().row_semantics().is_unknown());
        assert!(matches!(outcome, RelationalOutcome::Supported(value) if value.as_ref() == &query));
    }

    #[test]
    fn navigation_multiplicity_composes_receiver_and_member_bounds() {
        let exactly_one = Multiplicity::new(EXACTLY_ONE, Some(EXACTLY_ONE))
            .expect("fixture multiplicity must be valid");
        let optional =
            Multiplicity::new(0, Some(EXACTLY_ONE)).expect("fixture multiplicity must be valid");
        let empty = Multiplicity::new(0, Some(0)).expect("fixture multiplicity must be valid");

        assert_eq!(
            compose_navigation_multiplicity(optional, exactly_one),
            Some(optional)
        );
        assert_eq!(
            compose_navigation_multiplicity(optional, optional),
            Some(optional)
        );
        assert_eq!(
            compose_navigation_multiplicity(empty, Multiplicity::zero_or_more()),
            Some(empty)
        );
    }

    #[test]
    fn navigation_proofs_reject_qualified_members_and_mismatched_receivers() {
        let (graph, person) = navigation_fixture();
        let manager_chain = navigation_chain(&graph, &person, "manager");
        let navigation = ResolvedNavigation::from_chain(&manager_chain)
            .expect("plain to-one property must produce a navigation proof");
        let target = navigation.member().target().clone();
        let multiplicity = navigation.member().multiplicity();
        let member_origin = ModelOrigin::from_member(navigation.member());

        let input_column = Column::new(
            ColumnId::new(FIRST_COLUMN),
            Name::new("unprovenReceiver").expect("fixture name must be valid"),
            TypeRef::new(person.path().clone(), Vec::new()),
            Multiplicity::new(EXACTLY_ONE, Some(EXACTLY_ONE))
                .expect("fixture multiplicity must be valid"),
            Nullability::Unknown,
            origin(),
        );
        let input_schema =
            RelationSchema::new(vec![input_column.clone()]).expect("fixture schema must be valid");
        let input = scan(input_schema);
        let scalar = ScalarExpression::new(
            ScalarOperator::Navigation {
                input: Box::new(scalar_column(&input_column)),
                navigation: Box::new(navigation),
            },
            target,
            multiplicity,
            Nullability::Unknown,
            Knowledge::unknown(),
            IrOrigin::new(origin().source(), vec![member_origin]),
        );
        let output_column = Column::new(
            ColumnId::new(SECOND_COLUMN),
            Name::new("manager").expect("fixture name must be valid"),
            scalar.type_ref().clone(),
            scalar.multiplicity(),
            scalar.nullability(),
            origin(),
        );
        let output_schema =
            RelationSchema::new(vec![output_column.clone()]).expect("fixture schema must be valid");
        let invalid_receiver = RelationExpression::new(
            RelationOperator::Project {
                input: Box::new(input),
                projections: vec![Projection::new(output_column.id(), scalar)],
            },
            output_schema,
            RelationFacts::unknown(),
            origin(),
        );
        let qualified_chain = navigation_chain(&graph, &person, "zero");
        let to_many_chain = navigation_chain(&graph, &person, "reports");

        assert_eq!(
            invalid_receiver,
            Err(RelationExpressionError::NavigationMetadataMismatch)
        );
        assert!(ResolvedNavigation::from_chain(&qualified_chain).is_none());
        assert!(ResolvedNavigation::from_chain(&to_many_chain).is_none());
    }

    #[test]
    fn navigation_requires_member_provenance_on_its_result() {
        let (graph, person) = navigation_fixture();
        let chain = navigation_chain(&graph, &person, "manager");
        let navigation = ResolvedNavigation::from_chain(&chain)
            .expect("plain to-one property must produce a navigation proof");
        let multiplicity = navigation.member().multiplicity();
        let target = navigation.member().target().clone();
        let receiver_origin =
            IrOrigin::new(origin().source(), vec![ModelOrigin::from_class(&person)]);
        let input_column = Column::new(
            ColumnId::new(FIRST_COLUMN),
            Name::new("person").expect("fixture name must be valid"),
            TypeRef::new(person.path().clone(), Vec::new()),
            Multiplicity::new(EXACTLY_ONE, Some(EXACTLY_ONE))
                .expect("fixture multiplicity must be valid"),
            Nullability::Unknown,
            receiver_origin.clone(),
        );
        let input_schema =
            RelationSchema::new(vec![input_column.clone()]).expect("fixture schema must be valid");
        let input = scan(input_schema);
        let input_scalar = ScalarExpression::new(
            ScalarOperator::Column(input_column.id()),
            input_column.type_ref().clone(),
            input_column.multiplicity(),
            input_column.nullability(),
            Knowledge::unknown(),
            receiver_origin.clone(),
        );
        let scalar = ScalarExpression::new(
            ScalarOperator::Navigation {
                input: Box::new(input_scalar),
                navigation: Box::new(navigation),
            },
            target,
            multiplicity,
            Nullability::Unknown,
            Knowledge::unknown(),
            receiver_origin,
        );
        let output_column = Column::new(
            ColumnId::new(SECOND_COLUMN),
            Name::new("manager").expect("fixture name must be valid"),
            scalar.type_ref().clone(),
            scalar.multiplicity(),
            scalar.nullability(),
            scalar.origin().clone(),
        );
        let result = RelationExpression::new(
            RelationOperator::Project {
                input: Box::new(input),
                projections: vec![Projection::new(output_column.id(), scalar)],
            },
            RelationSchema::new(vec![output_column]).expect("fixture schema must be valid"),
            RelationFacts::unknown(),
            origin(),
        );

        assert_eq!(
            result,
            Err(RelationExpressionError::NavigationMetadataMismatch)
        );
    }

    #[test]
    fn navigation_requires_its_proven_output_multiplicity() {
        let (graph, person) = navigation_fixture();
        let chain = navigation_chain(&graph, &person, "manager");
        let navigation = ResolvedNavigation::from_chain(&chain)
            .expect("plain to-one property must produce a navigation proof");
        let receiver_origin =
            IrOrigin::new(origin().source(), vec![ModelOrigin::from_class(&person)]);
        let input_column = Column::new(
            ColumnId::new(FIRST_COLUMN),
            Name::new("person").expect("fixture name must be valid"),
            TypeRef::new(person.path().clone(), Vec::new()),
            multiplicity(EXACTLY_ONE, Some(EXACTLY_ONE)),
            Nullability::Unknown,
            receiver_origin.clone(),
        );
        let input_schema =
            RelationSchema::new(vec![input_column.clone()]).expect("fixture schema must be valid");
        let input = scan(input_schema);
        let input_scalar = ScalarExpression::new(
            ScalarOperator::Column(input_column.id()),
            input_column.type_ref().clone(),
            input_column.multiplicity(),
            input_column.nullability(),
            Knowledge::unknown(),
            receiver_origin,
        );
        let output_multiplicity = multiplicity(0, Some(EXACTLY_ONE));
        let scalar = ScalarExpression::new(
            ScalarOperator::Navigation {
                input: Box::new(input_scalar),
                navigation: Box::new(navigation.clone()),
            },
            navigation.member().target().clone(),
            output_multiplicity,
            Nullability::Unknown,
            Knowledge::unknown(),
            IrOrigin::new(
                origin().source(),
                vec![ModelOrigin::from_member(navigation.member())],
            ),
        );
        let output_column = Column::new(
            ColumnId::new(SECOND_COLUMN),
            Name::new("manager").expect("fixture name must be valid"),
            scalar.type_ref().clone(),
            scalar.multiplicity(),
            scalar.nullability(),
            scalar.origin().clone(),
        );
        let result = RelationExpression::new(
            RelationOperator::Project {
                input: Box::new(input),
                projections: vec![Projection::new(output_column.id(), scalar)],
            },
            RelationSchema::new(vec![output_column]).expect("fixture schema must be valid"),
            RelationFacts::unknown(),
            origin(),
        );

        assert_eq!(
            result,
            Err(RelationExpressionError::NavigationMetadataMismatch)
        );
    }

    #[test]
    fn navigation_receiver_requires_exact_type_and_multiplicity() {
        let (graph, person) = navigation_fixture();
        let chain = navigation_chain(&graph, &person, "manager");
        let navigation = ResolvedNavigation::from_chain(&chain)
            .expect("plain to-one property must produce a navigation proof");
        let receiver_origin =
            IrOrigin::new(origin().source(), vec![ModelOrigin::from_class(&person)]);
        let member_origin = ModelOrigin::from_member(navigation.member());

        let wrong_type_column = Column::new(
            ColumnId::new(FIRST_COLUMN),
            Name::new("person").expect("fixture name must be valid"),
            string_type(),
            multiplicity(EXACTLY_ONE, Some(EXACTLY_ONE)),
            Nullability::Unknown,
            receiver_origin.clone(),
        );
        let wrong_type_schema = RelationSchema::new(vec![wrong_type_column.clone()])
            .expect("fixture schema must be valid");
        let wrong_type_input = ScalarExpression::new(
            ScalarOperator::Column(wrong_type_column.id()),
            wrong_type_column.type_ref().clone(),
            wrong_type_column.multiplicity(),
            wrong_type_column.nullability(),
            Knowledge::unknown(),
            receiver_origin.clone(),
        );
        let wrong_type = ScalarExpression::new(
            ScalarOperator::Navigation {
                input: Box::new(wrong_type_input),
                navigation: Box::new(navigation.clone()),
            },
            navigation.member().target().clone(),
            navigation.member().multiplicity(),
            Nullability::Unknown,
            Knowledge::unknown(),
            IrOrigin::new(origin().source(), vec![member_origin.clone()]),
        );

        let wrong_multiplicity = multiplicity(0, Some(EXACTLY_ONE));
        let wrong_multiplicity_column = Column::new(
            ColumnId::new(FIRST_COLUMN),
            Name::new("person").expect("fixture name must be valid"),
            TypeRef::new(person.path().clone(), Vec::new()),
            wrong_multiplicity,
            Nullability::Unknown,
            receiver_origin.clone(),
        );
        let wrong_multiplicity_schema =
            RelationSchema::new(vec![wrong_multiplicity_column.clone()])
                .expect("fixture schema must be valid");
        let wrong_multiplicity_input = ScalarExpression::new(
            ScalarOperator::Column(wrong_multiplicity_column.id()),
            wrong_multiplicity_column.type_ref().clone(),
            wrong_multiplicity_column.multiplicity(),
            wrong_multiplicity_column.nullability(),
            Knowledge::unknown(),
            receiver_origin,
        );
        let wrong_multiplicity_scalar = ScalarExpression::new(
            ScalarOperator::Navigation {
                input: Box::new(wrong_multiplicity_input),
                navigation: Box::new(navigation.clone()),
            },
            navigation.member().target().clone(),
            wrong_multiplicity,
            Nullability::Unknown,
            Knowledge::unknown(),
            IrOrigin::new(origin().source(), vec![member_origin]),
        );

        assert_eq!(
            validate_scalar(&wrong_type, &[&wrong_type_schema]),
            Err(RelationExpressionError::NavigationMetadataMismatch)
        );
        assert_eq!(
            validate_scalar(&wrong_multiplicity_scalar, &[&wrong_multiplicity_schema]),
            Err(RelationExpressionError::NavigationMetadataMismatch)
        );
    }
}
