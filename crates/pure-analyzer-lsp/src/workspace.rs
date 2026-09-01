use serde_json::Value;

/// The front-end workspace-configuration boundary.
///
/// Configuration remains an opaque protocol value until a later adapter maps
/// it to an analysis configuration. That keeps LSP transport types out of the
/// core crates.
#[derive(Debug, Default)]
pub struct WorkspaceConfiguration {
    settings: Option<Value>,
}

impl WorkspaceConfiguration {
    /// Replace the most recently received client workspace settings.
    pub fn replace(&mut self, settings: Value) {
        self.settings = Some(settings);
    }

    /// Return the current raw client workspace settings, if any.
    #[must_use]
    pub const fn settings(&self) -> Option<&Value> {
        self.settings.as_ref()
    }
}
