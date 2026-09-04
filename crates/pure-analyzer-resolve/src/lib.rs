#![forbid(unsafe_code)]
#![deny(missing_docs)]

//! Deterministic model-name and member resolution.
//!
//! Resolution is deliberately non-fatal. A loaded model can be incomplete or
//! contain a broken generalization graph, and callers need a typed outcome to
//! decide whether a downstream finding is conclusive.

use std::collections::BTreeSet;

use pure_analyzer_diagnostics::TextRange;

mod local;

pub use local::{
    LocalValue, LocalValueKind, NavigationAmbiguity, NavigationArityMismatch, NavigationChain,
    NavigationCycle, NavigationFailure, NavigationHop, NavigationMissing, NavigationResolution,
    NavigationResolver, NavigationStep, NavigationTarget, NavigationUnderResolution,
    NavigationUnderResolutionReason, RelationColumn, RelationColumnId, RelationRow,
    RelationRowError, TypeEnvironment, TypeScope, UnknownValue,
};
use pure_analyzer_model::{
    ClassId, ClassInfo, ModelGraph, Multiplicity, Name, Provenance, QName, QpInfo, QpKind,
    SourceId, Temporal, TypeRef,
};

/// Source location information for a resolved definition.
///
/// PMCD currently identifies a source document but does not preserve element
/// offsets, so its anchors have [`None`] spans. The optional span keeps the
/// result shape truthful and ready for source-backed model facts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DefinitionAnchor {
    source: SourceId,
    span: Option<TextRange>,
}

impl DefinitionAnchor {
    /// Construct an anchor from the owning source and an optional precise span.
    #[must_use]
    pub const fn new(source: SourceId, span: Option<TextRange>) -> Self {
        Self { source, span }
    }

    /// Return the source that supplied the winning definition.
    #[must_use]
    pub const fn source(self) -> SourceId {
        self.source
    }

    /// Return the definition span when the model source preserves one.
    #[must_use]
    pub const fn span(self) -> Option<TextRange> {
        self.span
    }
}

/// Stable facts about a class selected by resolution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedClass {
    id: ClassId,
    path: QName,
    temporal: Option<Temporal>,
    provenance: Provenance,
    definition: DefinitionAnchor,
}

impl ResolvedClass {
    /// Return the graph-stable class identifier.
    #[must_use]
    pub const fn id(&self) -> ClassId {
        self.id
    }

    /// Return the fully-qualified class path.
    #[must_use]
    pub const fn path(&self) -> &QName {
        &self.path
    }

    /// Return the class's directly declared temporal stereotype.
    #[must_use]
    pub const fn temporal(&self) -> Option<Temporal> {
        self.temporal
    }

    /// Return the model-source provenance of the class definition.
    #[must_use]
    pub const fn provenance(&self) -> Provenance {
        self.provenance
    }

    /// Return the source and optional span of the class definition.
    #[must_use]
    pub const fn definition(&self) -> DefinitionAnchor {
        self.definition
    }
}

/// Category of an effective resolved member.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolvedMemberKind {
    /// A qualified property, including generated milestoning properties.
    Qualified(QpKind),
    /// A plain class property.
    Property,
    /// A navigation end materialized from an association.
    AssociationEnd {
        /// The association that contributed this navigation end.
        association: QName,
    },
}

/// Stable facts about a member selected by resolution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedMember {
    owner: ResolvedClass,
    name: Name,
    target: TypeRef,
    multiplicity: Multiplicity,
    kind: ResolvedMemberKind,
    signature: Option<Vec<TypeRef>>,
    target_temporal_arity: Option<u8>,
    provenance: Provenance,
    definition: DefinitionAnchor,
}

impl ResolvedMember {
    /// Return the class that declares the selected member.
    #[must_use]
    pub const fn owner(&self) -> &ResolvedClass {
        &self.owner
    }

    /// Return the simple name of the selected member.
    #[must_use]
    pub const fn name(&self) -> &Name {
        &self.name
    }

    /// Return the member's target type, including generic arguments.
    #[must_use]
    pub const fn target(&self) -> &TypeRef {
        &self.target
    }

    /// Return the member's declared multiplicity.
    #[must_use]
    pub const fn multiplicity(&self) -> Multiplicity {
        self.multiplicity
    }

    /// Return whether the selected member is qualified, plain, or association-derived.
    #[must_use]
    pub const fn kind(&self) -> &ResolvedMemberKind {
        &self.kind
    }

    /// Return the qualified-property parameter types, when this is a qualified property.
    #[must_use]
    pub fn signature(&self) -> Option<&[TypeRef]> {
        self.signature.as_deref()
    }

    /// Return the effective temporal argument arity of the member's target, when known.
    ///
    /// A target whose whole reachable hierarchy is present, claims complete
    /// coverage, and carries no temporal stereotype is conclusively zero-arity.
    /// A missing value means instead that the target is outside the loaded
    /// graph, has conflicting temporal facts through generalization, or sits in
    /// a hierarchy whose coverage is open, leaving its stereotype undetermined.
    /// Without an association temporal overlay, a broken target hierarchy
    /// produces an under-resolved or cycle lookup outcome.
    #[must_use]
    pub const fn target_temporal_arity(&self) -> Option<u8> {
        self.target_temporal_arity
    }

    /// Return the provenance of the selected definition.
    #[must_use]
    pub const fn provenance(&self) -> Provenance {
        self.provenance
    }

    /// Return the source and optional span of the selected definition.
    #[must_use]
    pub const fn definition(&self) -> DefinitionAnchor {
        self.definition
    }
}

/// Why an otherwise valid lookup cannot make a closed-world conclusion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnderResolution {
    /// A class names a supertype that is absent from the loaded model graph.
    MissingSupertype {
        /// Class whose declaration references the missing supertype.
        owner: QName,
        /// Supertype path absent from the graph.
        missing: QName,
    },
    /// A source does not claim complete member coverage for the named class.
    OpenWorld {
        /// Class whose source leaves member coverage open.
        class: QName,
    },
    /// A normalized graph invariant required for stable resolution is absent.
    GraphInvariant {
        /// Class whose graph index entry is missing.
        class: QName,
    },
    /// The generalization graph is deeper than the resolver's bounded walk.
    GeneralizationDepth {
        /// Class reached once the depth budget was exhausted.
        class: QName,
        /// Longest generalization chain the resolver walks.
        limit: usize,
    },
}

/// Typed outcome of model resolution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolution<T> {
    /// The lookup selected one stable result.
    Found(T),
    /// The requested class, identifier, or member is absent from a closed graph.
    Missing,
    /// Available model facts are insufficient to make a closed-world result.
    UnderResolved(UnderResolution),
    /// Multiple equally-preferred inherited definitions apply.
    Ambiguous(Vec<T>),
    /// The reachable generalization graph contains a cycle.
    Cycle(Vec<QName>),
}

/// Deterministic resolver over one normalized [`ModelGraph`].
#[derive(Debug, Clone, Copy)]
pub struct Resolver<'model> {
    graph: &'model ModelGraph,
}

impl<'model> Resolver<'model> {
    /// Construct a resolver over one immutable normalized model graph.
    #[must_use]
    pub const fn new(graph: &'model ModelGraph) -> Self {
        Self { graph }
    }

    /// Return the graph used by this resolver.
    #[must_use]
    pub const fn graph(&self) -> &'model ModelGraph {
        self.graph
    }

    /// Resolve a fully-qualified class path to its stable graph facts.
    #[must_use]
    pub fn resolve_class(&self, path: &QName) -> Resolution<ResolvedClass> {
        let Some(class) = self.graph.class(path.as_str()) else {
            return Resolution::Missing;
        };
        match self.resolved_class(path, class) {
            Ok(class) => Resolution::Found(class),
            Err(reason) => Resolution::UnderResolved(reason),
        }
    }

    /// Resolve a stable graph class identifier to its class facts.
    #[must_use]
    pub fn resolve_class_id(&self, id: ClassId) -> Resolution<ResolvedClass> {
        let Some(class) = self.graph.class_by_id(id) else {
            return Resolution::Missing;
        };
        self.resolve_class(class.path())
    }

    /// Resolve all reachable supertypes in stable breadth-first lexical order.
    ///
    /// The origin class is excluded. Missing supertypes and cycles are reported
    /// as outcomes instead of being silently skipped.
    #[must_use]
    pub fn generalizations(&self, origin: &QName) -> Resolution<Vec<ResolvedClass>> {
        if self.graph.class(origin.as_str()).is_none() {
            return Resolution::Missing;
        }
        let levels = match self.hierarchy_levels(origin) {
            Ok(levels) => levels,
            Err(fault) => return fault.into_resolution(),
        };
        let mut resolved = Vec::new();
        for level in levels.into_iter().skip(1) {
            for path in level {
                let Some(class) = self.graph.class(path.as_str()) else {
                    return Resolution::UnderResolved(UnderResolution::GraphInvariant {
                        class: path,
                    });
                };
                match self.resolved_class(&path, class) {
                    Ok(class) => resolved.push(class),
                    Err(reason) => return Resolution::UnderResolved(reason),
                }
            }
        }
        Resolution::Found(resolved)
    }

    /// Resolve a member through the class and its generalizations.
    ///
    /// The nearest inheritance level wins. Within one level, generated qualified
    /// properties win over user qualified properties, then plain properties, then
    /// association ends. Equally preferred definitions at one inherited level are
    /// returned as a canonical ambiguity rather than selected by parent order. A
    /// direct declaration wins without inspecting unrelated inheritance edges;
    /// an inherited lookup validates the reachable hierarchy before selecting a
    /// definition.
    #[must_use]
    pub fn resolve_member(&self, origin: &QName, name: &Name) -> Resolution<ResolvedMember> {
        let Some(origin_class) = self.graph.class(origin.as_str()) else {
            return Resolution::Missing;
        };
        if origin_class.coverage_gap() {
            return Resolution::UnderResolved(UnderResolution::OpenWorld {
                class: origin.clone(),
            });
        }
        match self.direct_member(origin, origin_class, name) {
            Ok(Some(candidate)) => {
                return match self.complete_member(candidate) {
                    Ok(member) => Resolution::Found(member),
                    Err(fault) => fault.into_resolution(),
                };
            }
            Ok(None) => {}
            Err(fault) => return fault.into_resolution(),
        }
        let levels = match self.hierarchy_levels(origin) {
            Ok(levels) => levels,
            Err(fault) => return fault.into_resolution(),
        };
        for level in levels.into_iter().skip(1) {
            let mut candidates = Vec::new();
            let mut open_world = None;
            for path in level {
                let Some(class) = self.graph.class(path.as_str()) else {
                    return Resolution::UnderResolved(UnderResolution::GraphInvariant {
                        class: path,
                    });
                };
                if class.coverage_gap() {
                    open_world.get_or_insert_with(|| UnderResolution::OpenWorld {
                        class: path.clone(),
                    });
                }
                match self.direct_member(&path, class, name) {
                    Ok(Some(candidate)) => candidates.push(candidate),
                    Ok(None) => {}
                    Err(fault) => return fault.into_resolution(),
                }
            }
            if let Some(reason) = open_world {
                return Resolution::UnderResolved(reason);
            }
            if let Some(best_priority) = candidates.iter().map(|candidate| candidate.priority).min()
            {
                let best = candidates
                    .into_iter()
                    .filter(|candidate| candidate.priority == best_priority)
                    .collect::<Vec<_>>();
                let mut best = match best
                    .into_iter()
                    .map(|candidate| self.complete_member(candidate))
                    .collect::<Result<Vec<_>, _>>()
                {
                    Ok(best) => best,
                    Err(fault) => return fault.into_resolution(),
                };
                best.sort_by(|left, right| left.owner.path().cmp(right.owner.path()));
                if best.len() == 1 {
                    return Resolution::Found(best.remove(0));
                }
                return Resolution::Ambiguous(best);
            }
        }

        Resolution::Missing
    }

    fn hierarchy_levels(&self, origin: &QName) -> Result<Vec<Vec<QName>>, HierarchyFault> {
        self.validate_hierarchy(origin)?;
        let mut levels = vec![vec![origin.clone()]];
        let mut seen = BTreeSet::from([origin.clone()]);
        let mut current = vec![origin.clone()];

        while !current.is_empty() {
            let mut next = BTreeSet::new();
            for path in &current {
                let Some(class) = self.graph.class(path.as_str()) else {
                    return Err(HierarchyFault::UnderResolved(
                        UnderResolution::GraphInvariant {
                            class: path.clone(),
                        },
                    ));
                };
                for parent in sorted_supertypes(class) {
                    if seen.insert(parent.clone()) {
                        next.insert(parent);
                    }
                }
            }
            current = next.into_iter().collect();
            if !current.is_empty() {
                levels.push(current.clone());
            }
        }

        Ok(levels)
    }

    fn validate_hierarchy(&self, origin: &QName) -> Result<(), HierarchyFault> {
        let mut visited = BTreeSet::new();
        let mut stack = Vec::new();
        self.validate_node(origin, &mut visited, &mut stack)
    }

    fn validate_node(
        &self,
        current: &QName,
        visited: &mut BTreeSet<QName>,
        stack: &mut Vec<QName>,
    ) -> Result<(), HierarchyFault> {
        if stack.len() >= MAX_GENERALIZATION_DEPTH {
            return Err(HierarchyFault::UnderResolved(
                UnderResolution::GeneralizationDepth {
                    class: current.clone(),
                    limit: MAX_GENERALIZATION_DEPTH,
                },
            ));
        }
        if let Some(start) = stack.iter().position(|path| path == current) {
            let mut cycle = stack[start..].to_vec();
            cycle.push(current.clone());
            return Err(HierarchyFault::Cycle(cycle));
        }
        if !visited.insert(current.clone()) {
            return Ok(());
        }

        let Some(class) = self.graph.class(current.as_str()) else {
            return Err(HierarchyFault::UnderResolved(
                UnderResolution::GraphInvariant {
                    class: current.clone(),
                },
            ));
        };
        stack.push(current.clone());
        for parent in sorted_supertypes(class) {
            if self.graph.class(parent.as_str()).is_none() {
                return Err(HierarchyFault::UnderResolved(
                    UnderResolution::MissingSupertype {
                        owner: current.clone(),
                        missing: parent,
                    },
                ));
            }
            self.validate_node(&parent, visited, stack)?;
        }
        let _ = stack.pop();
        Ok(())
    }

    fn resolved_class(
        &self,
        path: &QName,
        class: &ClassInfo,
    ) -> Result<ResolvedClass, UnderResolution> {
        let Some(id) = self.graph.class_id(path.as_str()) else {
            return Err(UnderResolution::GraphInvariant {
                class: path.clone(),
            });
        };
        Ok(ResolvedClass {
            id,
            path: path.clone(),
            temporal: class.temporal(),
            provenance: class.provenance(),
            definition: DefinitionAnchor {
                source: class.source(),
                span: class.declaration_span(),
            },
        })
    }

    fn direct_member(
        &self,
        path: &QName,
        class: &ClassInfo,
        name: &Name,
    ) -> Result<Option<DirectMember>, HierarchyFault> {
        let owner = self
            .resolved_class(path, class)
            .map_err(HierarchyFault::UnderResolved)?;
        if let Some(property) = class.qualified_properties().get(name) {
            let priority = if property.kind() == QpKind::UserQualified {
                MemberPriority::UserQualified
            } else {
                MemberPriority::GeneratedQualified
            };
            return Ok(Some(DirectMember {
                priority,
                member: Self::qualified_member(owner, name, class, property),
                association: None,
            }));
        }
        let Some(property) = class.properties().get(name) else {
            return Ok(None);
        };
        let (priority, kind, provenance, definition, association) = if property.from_assoc() {
            let association = property.association().cloned();
            let (provenance, definition) = association
                .as_ref()
                .and_then(|path| {
                    self.graph
                        .associations()
                        .iter()
                        .find(|item| item.path() == path)
                })
                .map_or(
                    (
                        class.provenance(),
                        DefinitionAnchor {
                            source: class.source(),
                            span: property.declaration_span(),
                        },
                    ),
                    |item| {
                        (
                            item.provenance(),
                            DefinitionAnchor {
                                source: item.source(),
                                span: property.declaration_span(),
                            },
                        )
                    },
                );
            let kind = match association {
                Some(association) => ResolvedMemberKind::AssociationEnd { association },
                None => ResolvedMemberKind::Property,
            };
            (
                MemberPriority::AssociationEnd,
                kind,
                provenance,
                definition,
                property.association(),
            )
        } else {
            (
                MemberPriority::Property,
                ResolvedMemberKind::Property,
                class.provenance(),
                DefinitionAnchor {
                    source: class.source(),
                    span: property.declaration_span(),
                },
                None,
            )
        };
        Ok(Some(DirectMember {
            priority,
            member: ResolvedMember {
                owner,
                name: name.clone(),
                target: property.target().clone(),
                multiplicity: property.multiplicity(),
                kind,
                signature: None,
                target_temporal_arity: None,
                provenance,
                definition,
            },
            association: association.cloned(),
        }))
    }

    fn qualified_member(
        owner: ResolvedClass,
        name: &Name,
        class: &ClassInfo,
        property: &QpInfo,
    ) -> ResolvedMember {
        ResolvedMember {
            owner,
            name: name.clone(),
            target: property.target().clone(),
            multiplicity: property.multiplicity(),
            kind: ResolvedMemberKind::Qualified(property.kind()),
            signature: property.signature().map(<[TypeRef]>::to_vec),
            target_temporal_arity: None,
            provenance: class.provenance(),
            definition: DefinitionAnchor {
                source: class.source(),
                span: property.declaration_span(),
            },
        }
    }

    fn complete_member(&self, candidate: DirectMember) -> Result<ResolvedMember, HierarchyFault> {
        let mut member = candidate.member;
        member.target_temporal_arity =
            self.target_temporal_arity(&member.target, candidate.association.as_ref())?;
        Ok(member)
    }

    fn target_temporal_arity(
        &self,
        target: &TypeRef,
        association: Option<&QName>,
    ) -> Result<Option<u8>, HierarchyFault> {
        if let Some(arity) = association.and_then(|path| {
            self.graph
                .associations()
                .iter()
                .find(|item| item.path() == path)
                .and_then(|item| item.temporal())
                .map(Temporal::arity)
        }) {
            return Ok(Some(arity));
        }
        self.effective_temporal_arity(target.raw_type())
    }

    fn effective_temporal_arity(&self, target: &QName) -> Result<Option<u8>, HierarchyFault> {
        if self.graph.class(target.as_str()).is_none() {
            return Ok(None);
        }
        let levels = self.hierarchy_levels(target)?;
        for level in levels {
            let classes = level
                .iter()
                .filter_map(|path| self.graph.class(path.as_str()))
                .collect::<Vec<_>>();
            // `coverage_gap` is a coarse union: a stereotype the loader could
            // not read is indistinguishable here from one a parse gap swallowed
            // or from a merely incomplete member list. Telling them apart needs
            // a fact the model layer does not yet expose (#320), so this
            // withholds an answer for all of them and knowingly loses true
            // positives on gapped targets.
            if classes.iter().any(|class| class.coverage_gap()) {
                return Ok(None);
            }
            let temporal = classes
                .iter()
                .filter_map(|class| class.temporal())
                .collect::<BTreeSet<_>>();
            match temporal.len() {
                0 => {}
                1 => return Ok(temporal.iter().next().copied().map(Temporal::arity)),
                _ => return Ok(None),
            }
        }
        // The entire reachable hierarchy is present, claims complete member
        // coverage, and carries no temporal stereotype. That is a conclusive
        // zero-date answer, not an absence of facts: generated point
        // navigation to this target accepts no explicit temporal arguments.
        Ok(Some(0))
    }
}

/// Longest generalization chain the resolver walks before reporting an
/// under-resolution. Real Legend hierarchies are orders of magnitude shallower;
/// the budget exists so an untrusted model file cannot drive the depth-first
/// walk in `Resolver::validate_node` into a stack overflow. The parsers carry
/// their own budgets for the same hazard class; nothing couples the values.
const MAX_GENERALIZATION_DEPTH: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum MemberPriority {
    GeneratedQualified,
    UserQualified,
    Property,
    AssociationEnd,
}

#[derive(Debug, Clone)]
struct DirectMember {
    priority: MemberPriority,
    member: ResolvedMember,
    association: Option<QName>,
}

#[derive(Debug, Clone)]
enum HierarchyFault {
    UnderResolved(UnderResolution),
    Cycle(Vec<QName>),
}

impl HierarchyFault {
    fn into_resolution<T>(self) -> Resolution<T> {
        match self {
            Self::UnderResolved(reason) => Resolution::UnderResolved(reason),
            Self::Cycle(cycle) => Resolution::Cycle(cycle),
        }
    }
}

fn sorted_supertypes(class: &ClassInfo) -> Vec<QName> {
    let mut supertypes = class.supertypes().to_vec();
    supertypes.sort();
    supertypes.dedup();
    supertypes
}
