use std::collections::BTreeSet;

use serde_json::Value;

/// The JSON-RPC identifier of a cancellable request.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum RequestId {
    /// A string request identifier.
    String(String),
    /// An integer request identifier.
    Number(i64),
}

impl RequestId {
    pub(crate) fn from_json(value: &Value) -> Option<Self> {
        match value {
            Value::String(value) => Some(Self::String(value.clone())),
            Value::Number(value) => value.as_i64().map(Self::Number),
            _ => None,
        }
    }
}

/// The front-end-only cancellation boundary.
///
/// Analysis code receives cancellation decisions from its host rather than
/// depending on JSON-RPC request identifiers.
#[derive(Debug, Default)]
pub struct CancellationRegistry {
    cancelled: BTreeSet<RequestId>,
}

impl CancellationRegistry {
    /// Record one request as cancelled.
    pub fn cancel(&mut self, request: RequestId) {
        let _ = self.cancelled.insert(request);
    }

    /// Return whether a request has been cancelled.
    #[must_use]
    pub fn is_cancelled(&self, request: &RequestId) -> bool {
        self.cancelled.contains(request)
    }

    /// Return the number of remembered cancellations.
    #[must_use]
    pub fn len(&self) -> usize {
        self.cancelled.len()
    }

    /// Return whether no request is currently marked cancelled.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.cancelled.is_empty()
    }
}
