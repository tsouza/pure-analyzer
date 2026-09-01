use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

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
#[derive(Clone, Debug, Default)]
pub struct CancellationRegistry {
    active: Arc<Mutex<BTreeMap<RequestId, CancellationToken>>>,
}

/// The front-end-only cancellation state for one in-flight request.
#[derive(Clone, Debug)]
pub(crate) struct CancellationToken {
    cancelled: Arc<AtomicBool>,
}

impl CancellationToken {
    fn new() -> Self {
        Self {
            cancelled: Arc::new(AtomicBool::new(false)),
        }
    }

    pub(crate) fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }

    fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    fn same_request(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.cancelled, &other.cancelled)
    }
}

impl CancellationRegistry {
    /// Register one request before its worker begins.
    ///
    /// `None` rejects a duplicate in-flight JSON-RPC identifier, which keeps
    /// cancellation and result ownership unambiguous.
    pub(crate) fn begin(&self, request: RequestId) -> Option<CancellationToken> {
        self.with_active(|active| {
            if active.contains_key(&request) {
                return None;
            }
            let token = CancellationToken::new();
            let _ = active.insert(request, token.clone());
            Some(token)
        })
    }

    /// Mark an active request as cancelled.
    ///
    /// Unknown or already-completed identifiers are deliberately ignored: a
    /// cancellation notification cannot poison a later reuse of an identifier.
    pub fn cancel(&self, request: RequestId) {
        self.with_active(|active| active.get(&request).cloned())
            .as_ref()
            .map(CancellationToken::cancel);
    }

    /// Return whether an active request has been cancelled.
    #[must_use]
    pub fn is_cancelled(&self, request: &RequestId) -> bool {
        self.with_active(|active| active.get(request).cloned())
            .is_some_and(|token| token.is_cancelled())
    }

    /// Finish the matching request and return its final cancellation state.
    pub(crate) fn finish(&self, request: &RequestId, token: &CancellationToken) -> bool {
        self.with_active(|active| {
            if !active
                .get(request)
                .is_some_and(|active_token| active_token.same_request(token))
            {
                return false;
            }
            let _ = active.remove(request);
            token.is_cancelled()
        })
    }

    /// Return the number of active cancellable requests.
    #[must_use]
    pub fn len(&self) -> usize {
        self.with_active(|active| active.len())
    }

    /// Return whether no request is currently active.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.with_active(|active| active.is_empty())
    }

    fn with_active<T>(
        &self,
        access: impl FnOnce(&mut BTreeMap<RequestId, CancellationToken>) -> T,
    ) -> T {
        let mut active = self
            .active
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        access(&mut active)
    }
}
