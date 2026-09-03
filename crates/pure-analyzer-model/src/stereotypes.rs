//! Canonical spellings for the Legend Pure stereotype profiles this crate
//! interprets, and the qualified-property classification both loaders share.
//!
//! Legend Pure stereotypes are case-sensitive: the real engine only
//! recognizes `meta::pure::profiles::temporal` (or its `temporal` protocol
//! short form) applying exactly one of `bitemporal`, `businesstemporal`, or
//! `processingtemporal`, and `meta::pure::profiles::milestoning` applying
//! exactly `generatedmilestoningproperty`. Both the PMCD loader
//! ([`crate::loader`]) and the Pure-source loader ([`crate::pure`]) match
//! against these same constants so the two ingestion paths cannot silently
//! drift apart on non-canonical casing.

use crate::types::{Name, QpKind};

pub(crate) const TEMPORAL_PROFILE: &str = "meta::pure::profiles::temporal";
pub(crate) const TEMPORAL_PROFILE_PROTOCOL: &str = "temporal";
pub(crate) const MILESTONING_PROFILE: &str = "meta::pure::profiles::milestoning";
pub(crate) const MILESTONING_PROFILE_PROTOCOL: &str = "milestoning";

pub(crate) const BITEMPORAL: &str = "bitemporal";
pub(crate) const BUSINESS_TEMPORAL: &str = "businesstemporal";
pub(crate) const PROCESSING_TEMPORAL: &str = "processingtemporal";

pub(crate) const GENERATED_MILESTONING_PROPERTY: &str = "generatedmilestoningproperty";
pub(crate) const ALL_VERSIONS_SUFFIX: &str = "AllVersions";
pub(crate) const ALL_VERSIONS_IN_RANGE_SUFFIX: &str = "AllVersionsInRange";

/// Classifies a qualified property from its name and whether its source
/// marked it `generatedmilestoningproperty`.
///
/// Shared by the PMCD loader ([`crate::loader`]) and the Pure-source loader
/// ([`crate::pure`]), which each derive `generated` from their own source
/// shape (a JSON stereotype list vs. Pure-source stereotype annotations)
/// before calling this, so the classification rules themselves cannot drift
/// between the two ingestion paths.
///
/// Only the generated-name suffix distinguishes `AllVersions`/
/// `AllVersionsInRange` from a plain milestoned point navigation. Return
/// multiplicity is deliberately not a signal here: neither PMCD nor Pure
/// source carries any other engine-asserted milestoning-kind fact at this
/// point in lowering, and a to-many milestoned navigation (e.g. a generated
/// point-in-time property over a to-many association end) is still a point
/// navigation, not evidence of a structurally distinct "edge" shape (see
/// issue #300).
pub(crate) fn classify_qualified_property(name: &Name, generated: bool) -> QpKind {
    if generated && name.as_str().ends_with(ALL_VERSIONS_IN_RANGE_SUFFIX) {
        QpKind::AllVersionsInRange
    } else if generated && name.as_str().ends_with(ALL_VERSIONS_SUFFIX) {
        QpKind::AllVersions
    } else if generated {
        QpKind::MilestonedPoint
    } else {
        QpKind::UserQualified
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classification_has_explicit_precedence() {
        let orders = Name::new("orders").expect("valid");
        assert_eq!(
            classify_qualified_property(&orders, false),
            QpKind::UserQualified
        );
        assert_eq!(
            classify_qualified_property(&orders, true),
            QpKind::MilestonedPoint
        );

        let all_versions = Name::new("ordersAllVersions").expect("valid");
        assert_eq!(
            classify_qualified_property(&all_versions, true),
            QpKind::AllVersions
        );
        assert_eq!(
            classify_qualified_property(&all_versions, false),
            QpKind::UserQualified,
            "name suffixes alone must not synthesize a generated navigation"
        );

        let all_versions_in_range = Name::new("ordersAllVersionsInRange").expect("valid");
        assert_eq!(
            classify_qualified_property(&all_versions_in_range, true),
            QpKind::AllVersionsInRange
        );
    }

    #[test]
    fn to_many_generated_navigation_is_still_a_milestoned_point() {
        // Regression test for #300: `classify_qualified_property` takes no
        // multiplicity parameter at all, so a to-many generated navigation
        // (e.g. a generated point property over a to-many association end,
        // like `employees(): Person[*] {}`) cannot be misclassified from its
        // return arity the way `QpKind::EdgePoint` used to.
        let employees = Name::new("employees").expect("valid");
        assert_eq!(
            classify_qualified_property(&employees, true),
            QpKind::MilestonedPoint
        );
    }
}
