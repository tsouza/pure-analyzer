//! The committed schema-fixture database ids, shared via `#[path]` by the L2
//! lanes that iterate the whole in-scope corpus (soundness, properties). The
//! precision lane names its probe databases inline, so it does not include this
//! module — keeping every symbol here used by every lane that pulls it in.

/// The database ids that have a committed schema fixture (`tests/fixtures/schemas`).
/// The five arm-C pilot contexts plus the three out-of-sample (OOS) held-out
/// schemas — 269 in-scope gold queries in total (256 arm-A / 13 arm-C).
pub const FIXTURE_DBS: &[&str] = &[
    "battle_death",
    "car_1",
    "concert_singer",
    "employee_hire_evaluation",
    "pets_1",
    "dog_kennels",
    "student_transcripts_tracking",
    "world_1",
];
