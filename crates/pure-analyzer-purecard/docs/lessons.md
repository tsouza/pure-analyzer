# Lessons

The heuristics ledger. When the agent (or a reviewer) learns something that should
change future behavior but isn't yet worth a hard rule, it lands here — with
provenance, so we know why we believe it and how sure we are.

This file is **EVOLVABLE**. Entries flow in through the normal PR + reviewer loop.
The [self-learning methodology](methodology/self-learning.md) describes the loop;
this file is its memory. It is meant to stay **small**: lessons flow *out* as they
graduate into enforcement, so the ledger holds only what is still just judgment.

## How an entry works

Each lesson carries four things:

| Field          | Meaning                                                       |
| -------------- | ------------------------------------------------------------- |
| **Lesson**     | The heuristic, stated as an actionable rule of thumb.         |
| **Date**       | When it was first recorded (`YYYY-MM-DD`).                    |
| **Trigger**    | What made us learn it — the bug, review comment, or incident. |
| **Confidence** | `provisional` or `confirmed`.                                 |

## The N=3 promotion rule

A new lesson enters as **`provisional`**. Provisional lessons inform judgment but
are not treated as binding, and the reviewer applies them softly.

When the *same* lesson is independently re-triggered **three times** (N=3) —
three separate PRs or incidents where it would have prevented a problem — it is
promoted to **`confirmed`**. Update its row: bump the confidence, and append the
dates/triggers of the recurrences in the Trigger cell so the count is auditable.

**Graduation drains the lesson from this ledger.** A `confirmed` lesson does not
stay here as prose — its substance moves to the layer that can hold it best, and
its Active row is deleted:

- a **deterministic check** (a lint, an `ast-grep` rule, a test, a `just` gate)
  when the lesson is mechanically decidable — the flywheel in
  [quality-layers.md](methodology/quality-layers.md): judgment becomes free
  enforcement and the audit surface **shrinks**;
- a **constitution rule** ([constitution.md](../constitution.md)) when the lesson
  is a guardrail or craft principle no single check captures;
- a **methodology doc** when it's guidance about how we work.

All that remains here is a one-line pointer under "Graduated" so the flywheel
stays auditable; the full statement lives in its new home and this file's git
history. A clear-cut lesson may graduate straight to its home without waiting out
N=3 when a human calls it.

Retire a lesson (move it to "Retired") if it is contradicted by a later lesson or
made obsolete by a design change. Never silently delete — provenance is the point.

---

## Active lessons

| Lesson                                                                | Date | Trigger | Confidence |
| --------------------------------------------------------------------- | ---- | ------- | ---------- |
| *(none — every recorded lesson has graduated to its home; see below)* |      |         |            |

## Graduated

Drained into their enforcing home; kept only as one-line pointers so the flywheel
stays auditable. Full context lives in the linked home and this file's git history.

- **A one-tool recipe stays a plain `just` line; only real control flow moves to `cargo xtask`** (a nested `cargo xtask` → `cargo <plugin>` call can mangle the plugin's argv) → [constitution.md](../constitution.md) §2. (2026-07-05)
- **Verify a pin's current version at write time (`cargo add`), don't trust memory or lean on Dependabot as the currency mechanism** → [constitution.md](../constitution.md) §2 (Latest stable, verified). (2026-07-05)
- **The toolchain is declared once in `rust-toolchain.toml`; CI doesn't re-declare it with a `dtolnay/rust-toolchain` step** → [constitution.md](../constitution.md) §1. (2026-07-05)
- **commitlint `body-max-line-length` is unsatisfiable for Dependabot's generated body** → disabled in `commitlint.config.mjs`. (2026-07-05)
- **Gates run pre-merge and reproduce in CI** (post-merge-only automation needs a pre-merge counterpart; a gate mustn't depend on state a PR's detached-`HEAD` checkout lacks) → [constitution.md](../constitution.md) §2, enforced by `just release-plz-check`. (2026-07-05)
- **A branch must descend from current `origin/main` before a PR** → [constitution.md](../constitution.md) §2. (2026-07-05)
- **Prefer an in-tree crate over a new pin; bespoke code owns its format's edge cases** → [constitution.md](../constitution.md) §4. (2026-07-05)
- **Automation must not shell out to a platform-specific binary** (a `date` shell-out slipped in because no rule guarded the class) → deterministic `ast-grep-rules/no-platform-shellout.yml`, backing [constitution.md](../constitution.md) §2 (Portable automation). (2026-07-06, rot-sweep L2)
- **A figure a doc cites as evidence is machine-asserted against its source, never hand-copied** (a hand-copied number silently rots when its source changes and nobody notices) → a check regenerates the figure and fails until the doc and its source agree — the pattern behind the generated config reference's drift test, [methodology/twelve-factor.md](methodology/twelve-factor.md). (2026-07-06, rot-sweep L2)
- **A safety-critical library default is pinned in code, never inherited** (a default that a version bump or an env var can flip silently downgrades the guarantee you depend on) → set the safety option explicitly and lint any builder constructed without it, so the guarantee can't regress, backing [constitution.md](../constitution.md) §5. (2026-07-08, spec review)
- **A declared git hook the onboarding never installs is not a gate** (the `commit-msg` → commitlint hook lived in `lefthook.yml`, but `mise install && mise run install-cargo-tools` never ran `lefthook install`, so a fresh clone had no hook — commit-message and other violations surfaced only in CI, not at commit time) → the onboarding (`scripts/install-cargo-tools.mjs`, run by `mise run install-cargo-tools`) now runs `lefthook install`, so setup can't leave the declared hooks unwired. (2026-07-10, self-compliance audit)
- **A host-supplied `Vocab` that cannot express a grammar-legal query breaks soundness *invisibly*** (the mask is computed over the wrong byte stream; the gold-soundness gate never observes it, because it measures the core's own byte concatenation, not the host's tokenization) → an opt-in tokenizer self-check (`src/selfcheck.rs`: `self_check`/`self_check_smoke`, a distinct `SelfCheckError`) round-trips canonical queries *through tokens* and fails loud on host-vocab drift; the full 5034-query round-trip is `tests/selfcheck_corpus.rs`. (2026-07-11, M5)
- **A recognizer's EOS/acceptance predicate is derived from the transition function, never a hand-maintained state list** (a hand-listed accepting-state set duplicates `step`'s knowledge and drifts the moment a terminal state is added) → `Pda::is_accepting` probes `step` with a value-boundary byte; strictly additive, so gold stays 5034/5034 → [ADR-0006](decisions/0006-fuzz-crate-excluded-and-is-accepting-derived-from-step.md). (2026-07-11, M5)
- **A fuzz/nightly-`unsafe` harness lives in a workspace-excluded crate, never in the `forbid(unsafe)` stable core** (libfuzzer generates `unsafe` + needs nightly) → the `fuzz/` crate is excluded like `lints`, driven by `cargo +nightly fuzz` and its own `fuzz.yml` → [ADR-0006](decisions/0006-fuzz-crate-excluded-and-is-accepting-derived-from-step.md). (2026-07-11, M5)
- **An L2 relation-column check (N6) may only narrow in an *explicitly* named relation scope, never the open `tableToTDS` scope** (arm-A `getString('Sex')`/`restrict('FacID')` reference raw table columns the model never emitted as a `project`/`groupBy` name; narrowing them against an emitted-names set would mask real gold tokens — a soundness bug on 256 arm-A queries) → the shipped N6 fires only after an establishing op *closes*, and only over the accumulated emitted-string superset. The trigger boundary is proven precisely by the 256 arm-A queries in `tests/l2_soundness.rs`: they emit raw `getString`/`restrict` columns in the open `tableToTDS` scope, so if N6 fired there it would mask those real gold tokens and redden the sweep — their green *is* the proof it stays inert until an establishing op closes. `docs/spec/schema.md` §6.5 N6. (2026-07-11, M3)
- **Module docs describe present invariants, not milestone status; status is single-sourced, not restated in every module header** (frozen self-description silently lies to readers and ordinary review misses it) → three deterministic anti-drift gates: `just lint-stale`, `just doctest`, and `just check-doc-facts`. (2026-07-11, doc-drift audit)
- **A construct the *target* dialect emits but the frozen gold corpus never held is seeded in a separate provenance-distinct corpus, not force-fitted into the gold file** (folding it into `gold_queries.jsonl` mixes lineages and churns the load-bearing 5,034 count across ~15 citations, the doc-facts gate, and the selfcheck round-trip — noise that buries the change and risks a stale-citation failure) → `corpus/modern_dialect_seeds.jsonl` + its own `tests/modern_dialect_soundness.rs` lane holds the modern-Legend seeds (the `%latest` milestoning literal; the `~` Relation/Function API), keeping the oracle-driven principle while the gold corpus stays frozen → [ADR-0007](decisions/0007-modern-dialect-seed-corpus.md). (2026-07-16, gap report G1/G2)
- **A whole emitted construct family can collapse to a *single* PDA state when the machine is a value-hub over-approximation** (the arm-R Relation/Function API — `project(~[…])`, `groupBy(~[…])`, window `extend(over(~…))`, `sort([ascending(~…)])`, `rename(~…)` — looked like five new productions, but every one is just the existing value-hub/lambda/bracket/reducer machinery reached through one new sigil) → adding `SawTilde` (`~` → `[` / `~ident` / `~'…'`) plus a `{`-after-`:` branch admitted all of arm-R; the residue (e.g. the `relAggSpec`-vs-`winAggSpec` colon asymmetry) is a §5.6 over-approximation the compiler oracle re-catches. Don't hand-encode a family the hubs already cover → [ADR-0008](decisions/0008-arm-r-relation-function-api.md). (2026-07-16, gap report G1)
- **Every lambda binder is re-captured at its `|`, or a re-used name inherits an earlier lambda's binding** (an arm-R map lambda binds after a colon, so a `groupBy` binder kept the class a prior `filter(x|…)` gave it and N1 unsoundly masked a projected column) → `docs/spec/schema.md` §6.4; pinned by `an_arm_r_map_lambda_binder_narrows_columns_after_a_preceding_filter` / `a_typed_value_binder_keeps_its_pre_colon_binding` (`src/schema/scope.rs`) and `arm_r_groupby_map_lambda_binder_does_not_mask_a_projected_column` (`tests/l2_precision.rs`). (2026-07-17, L2 gap report)
- **An incomplete-risk L2 narrowing set must be an accumulated *superset*, gated to the arm it was authored for** (arm-R `$row.Col` narrows to the emitted-column universe only once a `~[` latches arm-R, so a real column is never masked and an arm-A getter is never taken for a phantom column) → `docs/spec/schema.md` §6.4/§6.5; proven by the 256 arm-A gold (`l2_soundness`) and `arm_r_groupby_map_lambda_binder_masks_a_phantom_column` (`tests/l2_precision.rs`). (2026-07-17, precision upgrade)
- **Per-pipeline tracker state (bound class, arm, relation-row-ness) is lexically scoped to the lambda body, or a nested subquery leaks it and masks a valid outer token** — the corpus has no nested-subquery query, so this class is pinned directly, not by replay → `docs/spec/schema.md` §6.4; `tests/l2_precision.rs` (`a_shadowed_binder_is_restored_when_the_inner_arm_r_scope_closes`, `a_nested_arm_r_subquery_does_not_taint_the_outer_arm_a_pipeline`, `a_navigation_headed_arm_r_subquery_does_not_taint_the_outer_pipeline`) and `a_nested_pipeline_source_class_does_not_leak_to_an_outer_navigation` (`src/schema/scope.rs`). (2026-07-17, reviewer)
- **An L1 construct is admitted oracle-driven from a must-parse/must-reject pair, never guessed** (a proposed "must-mask" turned out legal Legend Pure; the real gap was quoted-member access `$x.'name'`) → `specs/l1-quoted-member-access.md`; `after_dot_admits_a_quoted_member_name` (`src/grammar/pda.rs`) + nested-subquery seeds in `corpus/modern_dialect_seeds.jsonl`. (2026-07-17, L1 grammar audit)
- **`L1 ⊇ gold` is a sample, not the grammar — soundness is pinned by a differential gate against the real Legend engine, not by review** (a reviewer judged legal Pure illegal, and `.5`/`1.5e3`/datetime-fractional-seconds literals were silently rejected) → an engine-labeled corpus replayed in CI asserts `engine-parse ⟹ L1-accept` modulo a documented divergence allowlist → `specs/l1-differential-oracle.md`, `docs/spec/grammar.md` §5.10; `tests/differential_l1.rs`, `just label-differential`. (2026-07-18, differential oracle)
- **An L2 narrowing rule must cover the token that *crosses* its lexeme boundary, not only the one that starts at the anchor** (byte-level BPE fuses the nav `.` with a property's first byte into one token, so N1/N6 read the mask before the dot and a fused `.<phantom>` slipped through) → a subtractive fused-nav pass, ident-*start*-gated so a leading-dot float stays sound → `docs/spec/schema.md` §6.5; `ScopeTracker::fused_nav_position`/`narrow_fused_into`, `tests/l2_precision.rs` (`n1_masks_a_phantom_property_fused_with_the_nav_dot`, `a_fused_navdot_float_operand_is_never_masked`, `arm_r_groupby_map_binder_narrows_a_fused_relation_column`). (2026-07-18, L2 BPE fusion bug report)
- **A corpus-derived gate must mirror the decoder's *full* member universe, not a naive subset** (a structural-corpus generator that took a class's legal first-chars from scalar properties alone flagged 503 false leaks — an association end like `domainAuthors` is a real navigable member, so `.d` is legitimately admitted) → the phantom/fusion universe is scalar properties **plus** association ends navigable from the class → `tests/support/spider_corpus.rs` (`navigable_members`), pinned by `tests/spider_corpus_replay.rs`. (2026-07-18, Spider corpus infusion)
- **A broad corpus gate ships green by pinning soundness at 100% and precision leaks in a counted, self-honest allowlist, never by weakening the corpus** (replaying ~31.6k derived Spider cases surfaced two real precision gaps — N1 identifier-termination `$x.<prefix-of-member>`, and N2 nav off a name that is both an association end and a scalar property) → each leaking case carries a `GapKind` tag, leaks ⟺ tags with exact counts, and a fixed gap reddens until its tag and count are removed → `tests/spider_corpus_replay.rs`; the two gaps await their own fix PRs. (2026-07-18, Spider corpus infusion)

## Retired

Lessons no longer in force, with the reason.

| Lesson       | Date retired | Reason |
| ------------ | ------------ | ------ |
| *(none yet)* |              |        |
