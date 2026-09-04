use std::collections::BTreeSet;

use serde_json::Value;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ModelDocumentKind {
    Pmcd,
    Pure,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ModelDocument {
    uri: String,
    kind: ModelDocumentKind,
}

impl ModelDocument {
    pub(crate) fn uri(&self) -> &str {
        &self.uri
    }

    pub(crate) const fn kind(&self) -> ModelDocumentKind {
        self.kind
    }
}

/// The front-end workspace-configuration boundary.
///
/// The raw settings remain owned by this protocol crate. The only analysis
/// routing setting interpreted here is `modelDocuments`: an ordered array of
/// `{ "uri": string, "kind": "pure" | "pmcd" }` objects. A route identifies an
/// already-open client snapshot; URI suffixes and filesystem paths never infer
/// its model type.
#[derive(Debug, Default)]
pub(crate) struct WorkspaceConfiguration {
    settings: Option<Value>,
    model_documents: Vec<ModelDocument>,
    revision: u64,
}

impl WorkspaceConfiguration {
    /// Replace the most recently received client workspace settings.
    pub(crate) fn replace(&mut self, settings: Value) {
        let _ = self.update(settings);
    }

    /// Return the current raw client workspace settings, if any.
    #[cfg(test)]
    #[must_use]
    pub(crate) const fn settings(&self) -> Option<&Value> {
        self.settings.as_ref()
    }

    pub(crate) fn model_documents(&self) -> &[ModelDocument] {
        &self.model_documents
    }

    pub(crate) const fn revision(&self) -> u64 {
        self.revision
    }

    fn update(&mut self, settings: Value) -> bool {
        if self.settings.as_ref() == Some(&settings) {
            return false;
        }
        let Some(revision) = self.revision.checked_add(1) else {
            return false;
        };
        self.model_documents = model_documents(&settings);
        self.settings = Some(settings);
        self.revision = revision;
        true
    }
}

fn model_documents(settings: &Value) -> Vec<ModelDocument> {
    let Some(routes) = settings.get("modelDocuments").and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut seen = BTreeSet::new();
    routes
        .iter()
        .filter_map(|route| {
            let uri = route.get("uri").and_then(Value::as_str)?;
            let kind = match route.get("kind").and_then(Value::as_str)? {
                "pmcd" => ModelDocumentKind::Pmcd,
                "pure" => ModelDocumentKind::Pure,
                _ => return None,
            };
            if uri.is_empty() || !seen.insert(uri.to_owned()) {
                return None;
            }
            Some(ModelDocument {
                uri: uri.to_owned(),
                kind,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use super::{ModelDocumentKind, WorkspaceConfiguration};

    #[test]
    fn workspace_configuration_distinguishes_initial_and_replaced_values() {
        let mut configuration = WorkspaceConfiguration::default();
        assert_eq!(configuration.settings(), None);
        configuration.replace(value(r#"{"maxProblems":20}"#));
        assert_eq!(
            configuration.settings(),
            Some(&value(r#"{"maxProblems":20}"#))
        );
    }

    #[test]
    fn accepts_only_explicit_unique_model_routes_in_configuration_order() {
        let mut configuration = WorkspaceConfiguration::default();
        configuration.replace(value(
            r#"{
                "modelDocuments": [
                    { "uri": "untitled:domain", "kind": "pure" },
                    { "uri": "file:///schema-without-heuristics", "kind": "pmcd" },
                    { "uri": "untitled:domain", "kind": "pmcd" },
                    { "uri": "file:///malformed", "kind": "unsupported" },
                    { "uri": 7, "kind": "pure" }
                ]
            }"#,
        ));

        let routes = configuration.model_documents();
        assert_eq!(routes.len(), 2);
        assert_eq!(routes[0].uri(), "untitled:domain");
        assert_eq!(routes[0].kind(), ModelDocumentKind::Pure);
        assert_eq!(routes[1].uri(), "file:///schema-without-heuristics");
        assert_eq!(routes[1].kind(), ModelDocumentKind::Pmcd);
    }

    fn value(source: &str) -> Value {
        serde_json::from_str(source).expect("test JSON must parse")
    }
}
