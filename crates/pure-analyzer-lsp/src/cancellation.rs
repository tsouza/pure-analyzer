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

#[cfg(test)]
mod tests {
    use super::{CancellationRegistry, RequestId};

    #[test]
    fn begin_registers_a_fresh_token_and_rejects_a_duplicate_active_identifier() {
        let registry = CancellationRegistry::default();
        let id = RequestId::Number(1);

        let token = registry.begin(id.clone()).expect("first begin succeeds");
        assert!(!token.is_cancelled());
        assert_eq!(registry.len(), 1);
        assert!(!registry.is_empty());

        assert!(
            registry.begin(id.clone()).is_none(),
            "a duplicate in-flight identifier must be rejected"
        );
        assert_eq!(
            registry.len(),
            1,
            "the rejected duplicate must not replace the original"
        );
    }

    #[test]
    fn finish_removes_the_entry_and_reports_the_final_cancellation_state() {
        let registry = CancellationRegistry::default();
        let id = RequestId::Number(2);

        let token = registry.begin(id.clone()).expect("begin succeeds");
        assert!(
            !registry.finish(&id, &token),
            "an uncancelled completion must report false"
        );
        assert!(registry.is_empty(), "finish must remove the entry");
        assert!(!registry.is_cancelled(&id));

        let token = registry
            .begin(id.clone())
            .expect("the identifier is reusable once its prior entry finished");
        registry.cancel(id.clone());
        assert!(token.is_cancelled());
        assert!(
            registry.finish(&id, &token),
            "a cancelled completion must report true"
        );
        assert!(registry.is_empty());
    }

    #[test]
    fn finish_ignores_a_token_that_does_not_match_the_active_registration() {
        let registry = CancellationRegistry::default();
        let first = RequestId::Number(3);
        let second = RequestId::String("other".to_owned());

        let first_token = registry.begin(first.clone()).expect("begin the first id");
        let second_token = registry.begin(second.clone()).expect("begin the second id");

        assert!(
            !registry.finish(&first, &second_token),
            "a token from a different request must not finish this one"
        );
        assert_eq!(
            registry.len(),
            2,
            "a mismatched token must leave the active entry in place"
        );

        assert!(!registry.finish(&first, &first_token));
        assert_eq!(registry.len(), 1);
        assert!(!registry.finish(&second, &second_token));
        assert!(registry.is_empty());
    }

    #[test]
    fn cancel_and_is_cancelled_ignore_identifiers_that_are_not_active() {
        let registry = CancellationRegistry::default();
        let id = RequestId::Number(4);

        assert!(!registry.is_cancelled(&id));
        registry.cancel(id.clone());
        assert!(!registry.is_cancelled(&id));
        assert!(registry.is_empty());
        assert_eq!(registry.len(), 0);
    }
}
