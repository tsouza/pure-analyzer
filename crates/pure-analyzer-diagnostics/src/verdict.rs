//! `eq`/`diff` verdicts and the `Indecisive` reason-code taxonomy (design doc
//! §5.3/§6.2).

/// The outcome of `eq`/`diff`: sound, incomplete, three-valued.
///
/// `eq`'s soundness is sacred (design doc §1.3): this type has no fourth
/// variant that could be mistaken for a commitment, and every producer must
/// map "don't know" to [`Verdict::Indecisive`] rather than guessing.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "verdict", rename_all = "snake_case")]
pub enum Verdict {
    /// Proven equal by canonical-normal-form match.
    Equivalent,
    /// Proven distinct by a concrete, model-legal witness.
    NotEquivalent {
        /// A rendered, paste-and-run-in-the-engine Pure literal exhibiting
        /// the divergence.
        witness: String,
    },
    /// Neither proof succeeded. The owning [`crate::Diagnostic`]'s `reason`
    /// field carries why (see [`ReasonCode`]).
    Indecisive,
}

/// Which side of the taxonomy an [`Indecisive`](Verdict::Indecisive) (or a
/// downgraded lint finding) falls on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReasonBucket {
    /// Out of scope by design (e.g. window/OLAP frame equivalence) — never
    /// planned to resolve without a fundamentally different technique (an
    /// SMT arm, in `pure-analyzer-eq-smt`).
    Fundamental,
    /// A known engineering gap that could be closed with more resolver/eq
    /// coverage; tracked as backlog, not a permanent limit.
    Recoverable,
}

/// A stable identifier explaining *why* a diagnostic is `Indecisive` or was
/// downgraded from Error to Warning under model under-resolution (e.g.
/// `MODEL_INCOMPLETE`, `IND_WINDOW`).
///
/// `id` and `blurb` are `&'static str` because every `ReasonCode` in the
/// codebase is a compile-time constant (design doc §6.1) — passes reference
/// one of a small closed set, they never construct one at runtime. That
/// makes this type intentionally `Serialize`-only: a `&'static str` field
/// cannot soundly implement `Deserialize` (it would require the input buffer
/// itself to live forever), and nothing in the design reads a `ReasonCode`
/// back out of rendered JSON — findings flow one way, from passes to
/// renderers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct ReasonCode {
    /// The stable id, e.g. `"MODEL_INCOMPLETE"`, `"IND_WINDOW"`.
    pub id: &'static str,
    /// Which side of the taxonomy this reason falls on.
    pub bucket: ReasonBucket,
    /// A one-line human explanation, shown in `explain` output.
    pub blurb: &'static str,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn not_equivalent_carries_its_witness() {
        let v = Verdict::NotEquivalent {
            witness: "#>{db.T}#->filter(x|$x.a == 1)".to_owned(),
        };
        assert!(matches!(v, Verdict::NotEquivalent { witness } if witness.contains("filter")));
    }

    #[test]
    fn verdict_serializes_with_a_tag() {
        let json = serde_json::to_string(&Verdict::Equivalent).expect("serialize");
        assert!(json.contains("\"verdict\":\"equivalent\""));
    }

    #[test]
    fn reason_code_serializes_its_fields() {
        const IND_WINDOW: ReasonCode = ReasonCode {
            id: "IND_WINDOW",
            bucket: ReasonBucket::Fundamental,
            blurb: "window/OLAP frame equivalence is out of scope",
        };
        let json = serde_json::to_string(&IND_WINDOW).expect("serialize");
        assert!(json.contains("IND_WINDOW"));
        assert!(json.contains("fundamental"));
    }
}
