//! Live Legend infrastructure assertions for issue #55's "pin/assert Legend
//! Engine/SDLC versions, health... so infrastructure failures are distinct
//! from compiler verdicts" bullet.
//!
//! Opt-in (`#[cfg(feature = "legend")]`): requires the pinned Legend stack
//! (`just test-legend` brings it up and tears it down).
#![cfg(feature = "legend")]
#![forbid(unsafe_code)]

use std::path::PathBuf;
use std::time::Duration;

#[path = "support/legend.rs"]
mod legend;

use legend::LegendClient;

const ENGINE_BASE: &str = "http://localhost:6300/api";
const HEALTH_TIMEOUT: Duration = Duration::from_secs(90);

/// The engine image tag pinned in `corpus/legend-stack/docker-compose.yml`
/// (`image: finos/legend-engine-server-http-server:X.Y.Z`), read from the
/// committed file rather than duplicated as a second constant here — a
/// version bump only has one place to change.
fn pinned_engine_version() -> String {
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("corpus/legend-stack/docker-compose.yml");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|err| panic!("read {}: {err}", path.display()));
    let needle = "image: finos/legend-engine-server-http-server:";
    let at = text
        .find(needle)
        .unwrap_or_else(|| panic!("{} has no pinned engine image line", path.display()));
    let rest = &text[at + needle.len()..];
    let end = rest.find(char::is_whitespace).unwrap_or_else(|| {
        panic!(
            "{} engine image line has no trailing whitespace",
            path.display()
        )
    });
    rest[..end].to_string()
}

/// The running engine container is exactly the version pinned in the
/// committed compose file — not `:latest`, not a stale locally-cached image
/// left over from a prior pin. `/server/v1/info`'s `legendSDLC.git.build.version`
/// field carries the engine's own build version (confirmed live: it reads
/// `"4.113.0"` against today's pin, matching `git.closest.tag.name`'s
/// `"legend-engine-4.113.0"`).
///
/// The SDLC container's own `/api/info` does not carry an equivalent field
/// for the `legend-sdlc-server-fs` image this stack pins (confirmed live:
/// its `platform.version` is `null`) — there is no live signal to assert
/// against for that half of the stack, so only the engine is checked here.
#[test]
fn the_running_engine_matches_the_pinned_compose_version() {
    let client = LegendClient::new(ENGINE_BASE);
    client
        .health_wait(HEALTH_TIMEOUT)
        .expect("Legend engine must become healthy");
    let info = client.info().expect("engine must answer /server/v1/info");
    let running = info["info"]["legendSDLC"]["git.build.version"]
        .as_str()
        .expect("info response has info.legendSDLC.git.build.version");
    assert_eq!(
        running,
        pinned_engine_version(),
        "the running engine's build version does not match the pinned compose image tag"
    );
}
