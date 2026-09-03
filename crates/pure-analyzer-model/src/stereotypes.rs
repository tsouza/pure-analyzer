//! Canonical spellings for the Legend Pure stereotype profiles this crate
//! interprets.
//!
//! Legend Pure stereotypes are case-sensitive: the real engine only
//! recognizes `meta::pure::profiles::temporal` (or its `temporal` protocol
//! short form) applying exactly one of `bitemporal`, `businesstemporal`, or
//! `processingtemporal`, and `meta::pure::profiles::milestoning` applying
//! exactly `generatedmilestoningproperty`. Both the PMCD loader
//! ([`crate::loader`]) and the Pure-source loader ([`crate::pure`]) match
//! against these same constants so the two ingestion paths cannot silently
//! drift apart on non-canonical casing.

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
