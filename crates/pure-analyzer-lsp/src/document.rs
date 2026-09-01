use std::collections::BTreeMap;

/// One complete, client-owned document snapshot.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DocumentSnapshot {
    uri: String,
    text: String,
    version: Option<i64>,
}

impl DocumentSnapshot {
    /// Construct a full document snapshot received from the protocol host.
    #[must_use]
    pub fn new(uri: String, text: String, version: Option<i64>) -> Self {
        Self { uri, text, version }
    }

    /// Return the document URI as supplied by the client.
    #[must_use]
    pub fn uri(&self) -> &str {
        &self.uri
    }

    /// Return the complete UTF-8 document text.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Return the client-assigned document version when supplied.
    #[must_use]
    pub const fn version(&self) -> Option<i64> {
        self.version
    }
}

/// The front-end document-store boundary.
///
/// The store deliberately retains complete client snapshots only. UTF-16
/// incremental synchronization is a later protocol concern and is not
/// interpreted by analysis-engine crates.
#[derive(Debug, Default)]
pub struct DocumentStore {
    documents: BTreeMap<String, DocumentSnapshot>,
}

impl DocumentStore {
    /// Insert or replace a complete document snapshot.
    pub fn insert(&mut self, document: DocumentSnapshot) {
        let _ = self.documents.insert(document.uri.clone(), document);
    }

    /// Remove one document by URI.
    pub fn remove(&mut self, uri: &str) -> Option<DocumentSnapshot> {
        self.documents.remove(uri)
    }

    /// Look up a document by URI.
    #[must_use]
    pub fn get(&self, uri: &str) -> Option<&DocumentSnapshot> {
        self.documents.get(uri)
    }

    /// Return the number of open documents.
    #[must_use]
    pub fn len(&self) -> usize {
        self.documents.len()
    }

    /// Return whether the store contains no documents.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.documents.is_empty()
    }
}
