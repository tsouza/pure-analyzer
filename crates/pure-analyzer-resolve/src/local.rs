//! Scoped local values and conservative navigation-chain resolution.

use std::collections::BTreeMap;

use pure_analyzer_diagnostics::{DiagCode, ReasonCode};
use pure_analyzer_model::{ModelGraph, Multiplicity, Name, QName, QpKind, TypeRef};

use crate::{
    DefinitionAnchor, Resolution, ResolvedClass, ResolvedMember, ResolvedMemberKind, Resolver,
    UnderResolution,
};

const NO_ARGUMENTS: usize = 0;
const RANGE_CONTEXT_ARGUMENTS: usize = 2;

/// A value whose local type can be tracked without full Pure inference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalValue {
    kind: LocalValueKind,
    multiplicity: Multiplicity,
}

impl LocalValue {
    /// Construct a locally known class value.
    #[must_use]
    pub fn class(class: ResolvedClass, multiplicity: Multiplicity) -> Self {
        Self {
            kind: LocalValueKind::Class(class),
            multiplicity,
        }
    }

    /// Construct a locally known scalar value.
    #[must_use]
    pub fn scalar(scalar: TypeRef, multiplicity: Multiplicity) -> Self {
        Self {
            kind: LocalValueKind::Scalar(scalar),
            multiplicity,
        }
    }

    /// Construct a locally known relation-row value.
    #[must_use]
    pub fn relation_row(row: RelationRow, multiplicity: Multiplicity) -> Self {
        Self {
            kind: LocalValueKind::RelationRow(row),
            multiplicity,
        }
    }

    /// Construct a value whose local type cannot be safely inferred.
    #[must_use]
    pub fn unknown(unknown: UnknownValue, multiplicity: Multiplicity) -> Self {
        Self {
            kind: LocalValueKind::Unknown(unknown),
            multiplicity,
        }
    }

    /// Return the tracked kind of this value.
    #[must_use]
    pub const fn kind(&self) -> &LocalValueKind {
        &self.kind
    }

    /// Return the tracked multiplicity of this value.
    #[must_use]
    pub const fn multiplicity(&self) -> Multiplicity {
        self.multiplicity
    }
}

/// The local category known for a value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LocalValueKind {
    /// A model class resolved through the deterministic model resolver.
    Class(ResolvedClass),
    /// A scalar type that the local environment does not navigate through.
    Scalar(TypeRef),
    /// One row of a relation with known column values.
    RelationRow(RelationRow),
    /// A value that local inference deliberately leaves unknown.
    Unknown(UnknownValue),
}

/// Why a local value has no safe concrete type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnknownValue {
    /// A higher-order flow has no locally trackable return type.
    HigherOrder,
    /// A named target is neither a loaded class nor an intrinsic scalar.
    UnmodeledType(TypeRef),
    /// Model facts needed to identify the target are incomplete.
    Model(UnderResolution),
}

/// A relation row with columns keyed by their exact Pure names.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RelationRow {
    columns: BTreeMap<Name, LocalValue>,
}

impl RelationRow {
    /// Construct a row from its known columns.
    #[must_use]
    pub fn new(columns: BTreeMap<Name, LocalValue>) -> Self {
        Self { columns }
    }

    /// Return all known columns in lexical name order.
    #[must_use]
    pub const fn columns(&self) -> &BTreeMap<Name, LocalValue> {
        &self.columns
    }

    /// Look up a column by its exact name.
    #[must_use]
    pub fn column(&self, name: &Name) -> Option<&LocalValue> {
        self.columns.get(name)
    }
}

/// A stack of lexical bindings for lambda parameters and `let` values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeEnvironment {
    root: BTreeMap<Name, LocalValue>,
    frames: Vec<BTreeMap<Name, LocalValue>>,
}

impl Default for TypeEnvironment {
    fn default() -> Self {
        Self::new()
    }
}

impl TypeEnvironment {
    /// Construct an empty root environment.
    #[must_use]
    pub fn new() -> Self {
        Self {
            root: BTreeMap::new(),
            frames: Vec::new(),
        }
    }

    /// Bind a value in the current lexical frame.
    ///
    /// A same-name binding in an inner frame shadows, but does not replace,
    /// the binding in an outer frame.
    pub fn bind(&mut self, name: Name, value: LocalValue) -> Option<LocalValue> {
        match self.frames.last_mut() {
            Some(frame) => frame.insert(name, value),
            None => self.root.insert(name, value),
        }
    }

    /// Look up the nearest visible binding for a name.
    #[must_use]
    pub fn lookup(&self, name: &Name) -> Option<&LocalValue> {
        self.frames
            .iter()
            .rev()
            .find_map(|frame| frame.get(name))
            .or_else(|| self.root.get(name))
    }

    /// Enter a child lexical scope that restores its outer bindings on drop.
    #[must_use]
    pub fn scope(&mut self) -> TypeScope<'_> {
        self.frames.push(BTreeMap::new());
        TypeScope { environment: self }
    }
}

/// A scoped mutable view of a [`TypeEnvironment`].
#[derive(Debug)]
pub struct TypeScope<'environment> {
    environment: &'environment mut TypeEnvironment,
}

impl TypeScope<'_> {
    /// Bind a value in this scope.
    pub fn bind(&mut self, name: Name, value: LocalValue) -> Option<LocalValue> {
        self.environment.bind(name, value)
    }

    /// Look up the nearest visible binding from this scope.
    #[must_use]
    pub fn lookup(&self, name: &Name) -> Option<&LocalValue> {
        self.environment.lookup(name)
    }

    /// Enter a nested lexical scope.
    #[must_use]
    pub fn scope(&mut self) -> TypeScope<'_> {
        self.environment.scope()
    }
}

impl Drop for TypeScope<'_> {
    fn drop(&mut self) {
        let _ = self.environment.frames.pop();
    }
}

/// One syntactic navigation step after a known local source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NavigationStep {
    name: Name,
    argument_count: usize,
}

impl NavigationStep {
    /// Construct a property-style navigation step with no arguments.
    #[must_use]
    pub const fn property(name: Name) -> Self {
        Self {
            name,
            argument_count: NO_ARGUMENTS,
        }
    }

    /// Construct a navigation step with the supplied source argument count.
    #[must_use]
    pub const fn call(name: Name, argument_count: usize) -> Self {
        Self {
            name,
            argument_count,
        }
    }

    /// Return the requested member or column name.
    #[must_use]
    pub const fn name(&self) -> &Name {
        &self.name
    }

    /// Return the number of arguments written at this step.
    #[must_use]
    pub const fn argument_count(&self) -> usize {
        self.argument_count
    }
}

/// One successful local navigation hop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NavigationHop {
    step: NavigationStep,
    target: NavigationTarget,
    value: LocalValue,
}

impl NavigationHop {
    /// Return the syntax-level step that produced this hop.
    #[must_use]
    pub const fn step(&self) -> &NavigationStep {
        &self.step
    }

    /// Return the resolved model member or relation column.
    #[must_use]
    pub const fn target(&self) -> &NavigationTarget {
        &self.target
    }

    /// Return the locally tracked value after this hop.
    #[must_use]
    pub const fn value(&self) -> &LocalValue {
        &self.value
    }

    /// Return the model definition anchor when this hop resolves a model member.
    #[must_use]
    pub const fn definition(&self) -> Option<DefinitionAnchor> {
        self.target.definition()
    }
}

/// The target selected by one navigation hop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NavigationTarget {
    /// A model property, qualified property, or association end.
    Member(ResolvedMember),
    /// A relation-row column bound by the current local scope.
    RelationColumn,
}

impl NavigationTarget {
    /// Return the model definition anchor when this target is a model member.
    #[must_use]
    pub const fn definition(&self) -> Option<DefinitionAnchor> {
        match self {
            Self::Member(member) => Some(member.definition()),
            Self::RelationColumn => None,
        }
    }
}

/// A completed prefix of a navigation chain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NavigationChain {
    source: LocalValue,
    value: LocalValue,
    hops: Vec<NavigationHop>,
}

impl NavigationChain {
    fn new(source: LocalValue) -> Self {
        Self {
            value: source.clone(),
            source,
            hops: Vec::new(),
        }
    }

    fn push(&mut self, step: NavigationStep, target: NavigationTarget, value: LocalValue) {
        self.hops.push(NavigationHop {
            step,
            target,
            value: value.clone(),
        });
        self.value = value;
    }

    /// Return the value at the start of this navigation chain.
    #[must_use]
    pub const fn source(&self) -> &LocalValue {
        &self.source
    }

    /// Return the value after the last completed hop.
    #[must_use]
    pub const fn value(&self) -> &LocalValue {
        &self.value
    }

    /// Return successful hops in source order.
    #[must_use]
    pub fn hops(&self) -> &[NavigationHop] {
        &self.hops
    }
}

/// A failed step together with the successfully resolved prefix before it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NavigationFailure {
    completed: NavigationChain,
    step: NavigationStep,
}

impl NavigationFailure {
    fn new(completed: NavigationChain, step: NavigationStep) -> Self {
        Self { completed, step }
    }

    /// Return the successful prefix before the failed step.
    #[must_use]
    pub const fn completed(&self) -> &NavigationChain {
        &self.completed
    }

    /// Return the step that did not resolve conclusively.
    #[must_use]
    pub const fn step(&self) -> &NavigationStep {
        &self.step
    }
}

/// A closed-world member or relation column miss.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NavigationMissing {
    failure: NavigationFailure,
}

impl NavigationMissing {
    /// Return the failed step and its successful prefix.
    #[must_use]
    pub const fn failure(&self) -> &NavigationFailure {
        &self.failure
    }
}

/// An equally preferred set of model members at one navigation step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NavigationAmbiguity {
    failure: NavigationFailure,
    candidates: Vec<ResolvedMember>,
}

impl NavigationAmbiguity {
    /// Return the failed step and its successful prefix.
    #[must_use]
    pub const fn failure(&self) -> &NavigationFailure {
        &self.failure
    }

    /// Return candidates in the resolver's deterministic order.
    #[must_use]
    pub fn candidates(&self) -> &[ResolvedMember] {
        &self.candidates
    }
}

/// A model generalization cycle reached while resolving one navigation step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NavigationCycle {
    failure: NavigationFailure,
    cycle: Vec<QName>,
}

impl NavigationCycle {
    /// Return the failed step and its successful prefix.
    #[must_use]
    pub const fn failure(&self) -> &NavigationFailure {
        &self.failure
    }

    /// Return the cycle path in resolver traversal order.
    #[must_use]
    pub fn cycle(&self) -> &[QName] {
        &self.cycle
    }
}

/// A conclusive mismatch between supplied and required navigation arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NavigationArityMismatch {
    failure: NavigationFailure,
    expected: usize,
    actual: usize,
    generated_milestoned: bool,
    definition: Option<DefinitionAnchor>,
}

impl NavigationArityMismatch {
    /// Return the failed step and its successful prefix.
    #[must_use]
    pub const fn failure(&self) -> &NavigationFailure {
        &self.failure
    }

    /// Return the required argument count at this step.
    #[must_use]
    pub const fn expected(&self) -> usize {
        self.expected
    }

    /// Return the argument count written at this step.
    #[must_use]
    pub const fn actual(&self) -> usize {
        self.actual
    }

    /// Return whether the resolved member is generated milestoned navigation.
    #[must_use]
    pub const fn is_generated_milestoned(&self) -> bool {
        self.generated_milestoned
    }

    /// Return the contributing model definition anchor when there is one.
    #[must_use]
    pub const fn definition(&self) -> Option<DefinitionAnchor> {
        self.definition
    }
}

/// A navigation result that is not soundly resolvable from available local facts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NavigationUnderResolution {
    /// No visible binding exists for a requested local variable.
    UnboundVariable {
        /// The missing lexical binding.
        name: Name,
    },
    /// A completed prefix reaches a source that cannot be navigated soundly.
    AtStep {
        /// The failed step and successful prefix.
        failure: Box<NavigationFailure>,
        /// The reason the next source or context is unknown.
        reason: Box<NavigationUnderResolutionReason>,
    },
}

impl NavigationUnderResolution {
    /// Return the registered diagnostic code for this under-resolution.
    #[must_use]
    pub const fn diagnostic_code(&self) -> DiagCode {
        DiagCode::UnknownSource
    }

    /// Return the stable downgraded-finding reason for this under-resolution.
    #[must_use]
    pub const fn reason_code(&self) -> ReasonCode {
        ReasonCode::ModelIncomplete
    }
}

/// The precise fact unavailable at an under-resolved navigation step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NavigationUnderResolutionReason {
    /// The source itself is deliberately unknown.
    UnknownValue(UnknownValue),
    /// A scalar does not carry enough model facts for property navigation.
    Scalar(TypeRef),
    /// Model lookup could not make a closed-world conclusion.
    Model(UnderResolution),
    /// A generated point-navigation member has no known temporal arity.
    TemporalArity(ResolvedMember),
    /// A user qualified property has no compiled parameter signature.
    QualifiedSignature(ResolvedMember),
}

/// The result of resolving a local navigation chain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NavigationResolution {
    /// Every requested step resolved to a locally tracked result.
    Found(NavigationChain),
    /// A known closed-world source does not contain the requested member or column.
    Missing(NavigationMissing),
    /// Available local or model facts cannot safely resolve the next step.
    UnderResolved(NavigationUnderResolution),
    /// Multiple equally-preferred model members apply at one step.
    Ambiguous(NavigationAmbiguity),
    /// A model generalization cycle prevents deterministic lookup.
    Cycle(NavigationCycle),
    /// The source is known but this step supplies the wrong number of arguments.
    WrongArity(NavigationArityMismatch),
}

/// Conservative local type and navigation resolver over one model graph.
#[derive(Debug, Clone, Copy)]
pub struct NavigationResolver<'model> {
    resolver: Resolver<'model>,
}

impl<'model> NavigationResolver<'model> {
    /// Construct a local navigation resolver over one immutable model graph.
    #[must_use]
    pub const fn new(graph: &'model ModelGraph) -> Self {
        Self {
            resolver: Resolver::new(graph),
        }
    }

    /// Return the underlying deterministic model resolver.
    #[must_use]
    pub const fn resolver(&self) -> Resolver<'model> {
        self.resolver
    }

    /// Infer the zero-or-more class value produced by `Class.all()`.
    #[must_use]
    pub fn class_all(&self, class: &QName) -> Resolution<LocalValue> {
        match self.resolver.resolve_class(class) {
            Resolution::Found(class) => {
                Resolution::Found(LocalValue::class(class, Multiplicity::zero_or_more()))
            }
            Resolution::Missing => Resolution::Missing,
            Resolution::UnderResolved(reason) => Resolution::UnderResolved(reason),
            Resolution::Ambiguous(classes) => Resolution::Ambiguous(
                classes
                    .into_iter()
                    .map(|class| LocalValue::class(class, Multiplicity::zero_or_more()))
                    .collect(),
            ),
            Resolution::Cycle(cycle) => Resolution::Cycle(cycle),
        }
    }

    /// Resolve a navigation chain beginning at a visible lexical binding.
    #[must_use]
    pub fn resolve_variable(
        &self,
        environment: &TypeEnvironment,
        name: &Name,
        steps: &[NavigationStep],
    ) -> NavigationResolution {
        let Some(value) = environment.lookup(name) else {
            return NavigationResolution::UnderResolved(
                NavigationUnderResolution::UnboundVariable { name: name.clone() },
            );
        };
        self.resolve(value, steps)
    }

    /// Resolve a navigation chain beginning at one explicit local value.
    #[must_use]
    pub fn resolve(&self, source: &LocalValue, steps: &[NavigationStep]) -> NavigationResolution {
        let mut chain = NavigationChain::new(source.clone());
        for step in steps {
            match self.resolve_step(chain.value(), step) {
                StepResolution::Found { target, value } => chain.push(step.clone(), target, value),
                StepResolution::Missing => {
                    return NavigationResolution::Missing(NavigationMissing {
                        failure: NavigationFailure::new(chain, step.clone()),
                    });
                }
                StepResolution::UnderResolved(reason) => {
                    return NavigationResolution::UnderResolved(
                        NavigationUnderResolution::AtStep {
                            failure: Box::new(NavigationFailure::new(chain, step.clone())),
                            reason: Box::new(reason),
                        },
                    );
                }
                StepResolution::Ambiguous(candidates) => {
                    return NavigationResolution::Ambiguous(NavigationAmbiguity {
                        failure: NavigationFailure::new(chain, step.clone()),
                        candidates,
                    });
                }
                StepResolution::Cycle(cycle) => {
                    return NavigationResolution::Cycle(NavigationCycle {
                        failure: NavigationFailure::new(chain, step.clone()),
                        cycle,
                    });
                }
                StepResolution::WrongArity {
                    expected,
                    generated_milestoned,
                    definition,
                } => {
                    return NavigationResolution::WrongArity(NavigationArityMismatch {
                        failure: NavigationFailure::new(chain, step.clone()),
                        expected,
                        actual: step.argument_count(),
                        generated_milestoned,
                        definition,
                    });
                }
            }
        }
        NavigationResolution::Found(chain)
    }

    fn resolve_step(&self, source: &LocalValue, step: &NavigationStep) -> StepResolution {
        match source.kind() {
            LocalValueKind::Class(class) => self.resolve_member_step(class, step),
            LocalValueKind::RelationRow(row) => self.resolve_relation_column(row, step),
            LocalValueKind::Scalar(scalar) => StepResolution::UnderResolved(
                NavigationUnderResolutionReason::Scalar(scalar.clone()),
            ),
            LocalValueKind::Unknown(unknown) => StepResolution::UnderResolved(
                NavigationUnderResolutionReason::UnknownValue(unknown.clone()),
            ),
        }
    }

    fn resolve_member_step(&self, class: &ResolvedClass, step: &NavigationStep) -> StepResolution {
        match self.resolver.resolve_member(class.path(), step.name()) {
            Resolution::Found(member) => self.resolve_member(member, step),
            Resolution::Missing => StepResolution::Missing,
            Resolution::UnderResolved(reason) => {
                StepResolution::UnderResolved(NavigationUnderResolutionReason::Model(reason))
            }
            Resolution::Ambiguous(candidates) => StepResolution::Ambiguous(candidates),
            Resolution::Cycle(cycle) => StepResolution::Cycle(cycle),
        }
    }

    fn resolve_member(&self, member: ResolvedMember, step: &NavigationStep) -> StepResolution {
        let expected = match expected_argument_count(&member) {
            Ok(expected) => expected,
            Err(reason) => return StepResolution::UnderResolved(*reason),
        };
        if step.argument_count() != expected {
            return StepResolution::WrongArity {
                expected,
                generated_milestoned: matches!(
                    member.kind(),
                    ResolvedMemberKind::Qualified(QpKind::MilestonedPoint | QpKind::EdgePoint)
                ),
                definition: Some(member.definition()),
            };
        }
        let value = self.infer_target(member.target(), member.multiplicity());
        StepResolution::Found {
            target: NavigationTarget::Member(member),
            value,
        }
    }

    fn resolve_relation_column(&self, row: &RelationRow, step: &NavigationStep) -> StepResolution {
        if step.argument_count() != NO_ARGUMENTS {
            return StepResolution::WrongArity {
                expected: NO_ARGUMENTS,
                generated_milestoned: false,
                definition: None,
            };
        }
        match row.column(step.name()) {
            Some(value) => StepResolution::Found {
                target: NavigationTarget::RelationColumn,
                value: value.clone(),
            },
            None => StepResolution::Missing,
        }
    }

    fn infer_target(&self, target: &TypeRef, multiplicity: Multiplicity) -> LocalValue {
        match self.resolver.resolve_class(target.raw_type()) {
            Resolution::Found(class) => LocalValue::class(class, multiplicity),
            Resolution::Missing if is_intrinsic_scalar(target.raw_type()) => {
                LocalValue::scalar(target.clone(), multiplicity)
            }
            Resolution::Missing => {
                LocalValue::unknown(UnknownValue::UnmodeledType(target.clone()), multiplicity)
            }
            Resolution::UnderResolved(reason) => {
                LocalValue::unknown(UnknownValue::Model(reason), multiplicity)
            }
            Resolution::Ambiguous(_) | Resolution::Cycle(_) => {
                LocalValue::unknown(UnknownValue::UnmodeledType(target.clone()), multiplicity)
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum StepResolution {
    Found {
        target: NavigationTarget,
        value: LocalValue,
    },
    Missing,
    UnderResolved(NavigationUnderResolutionReason),
    Ambiguous(Vec<ResolvedMember>),
    Cycle(Vec<QName>),
    WrongArity {
        expected: usize,
        generated_milestoned: bool,
        definition: Option<DefinitionAnchor>,
    },
}

fn expected_argument_count(
    member: &ResolvedMember,
) -> Result<usize, Box<NavigationUnderResolutionReason>> {
    match member.kind() {
        ResolvedMemberKind::Property | ResolvedMemberKind::AssociationEnd { .. } => {
            Ok(NO_ARGUMENTS)
        }
        ResolvedMemberKind::Qualified(QpKind::UserQualified) => {
            member.signature().map(<[TypeRef]>::len).ok_or_else(|| {
                Box::new(NavigationUnderResolutionReason::QualifiedSignature(
                    member.clone(),
                ))
            })
        }
        ResolvedMemberKind::Qualified(QpKind::AllVersions) => Ok(NO_ARGUMENTS),
        ResolvedMemberKind::Qualified(QpKind::AllVersionsInRange) => Ok(RANGE_CONTEXT_ARGUMENTS),
        ResolvedMemberKind::Qualified(QpKind::MilestonedPoint | QpKind::EdgePoint) => member
            .target_temporal_arity()
            .map(usize::from)
            .ok_or_else(|| {
                Box::new(NavigationUnderResolutionReason::TemporalArity(
                    member.clone(),
                ))
            }),
    }
}

fn is_intrinsic_scalar(path: &QName) -> bool {
    matches!(
        path.as_str(),
        "Any"
            | "Boolean"
            | "Date"
            | "DateTime"
            | "Decimal"
            | "Float"
            | "Integer"
            | "LatestDate"
            | "Number"
            | "StrictDate"
            | "String"
    )
}
