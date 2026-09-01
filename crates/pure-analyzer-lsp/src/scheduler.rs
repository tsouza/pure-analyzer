//! Front-end-only scheduling for cancellable LSP requests.

use std::{
    io::{self, Write},
    panic::{AssertUnwindSafe, catch_unwind},
    sync::mpsc::Sender,
    thread,
};

#[cfg(test)]
use std::sync::{
    Arc, Mutex,
    mpsc::{self, Receiver, SyncSender},
};

use serde_json::Value;

use crate::{
    RequestId, Server,
    cancellation::CancellationToken,
    response::{send_error, send_result},
    server::ServerEvent,
    state::{RequestCompletion, RequestWork},
};

const REQUEST_CANCELLED_CODE: i64 = -32_800;
const INTERNAL_ERROR_CODE: i64 = -32_603;

/// Test-only pauses around detached work execution.
///
/// The barrier is deliberately unavailable outside crate tests. It only gives
/// transcripts a causal ordering around the normal worker path; cancellation
/// and result-commit decisions remain unchanged.
#[cfg(test)]
#[derive(Debug)]
pub(crate) struct RequestTestBarrier {
    blocked: RequestId,
    events: mpsc::Sender<RequestTestEvent>,
    release: Mutex<Receiver<()>>,
}

/// A synchronization point reached by a selected test worker.
#[cfg(test)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RequestTestEvent {
    /// The worker owns a detached request snapshot but has not executed it.
    SnapshotCaptured,
    /// Analysis has completed but the worker has not sent its completion.
    WorkCompleted,
}

#[cfg(test)]
impl RequestTestBarrier {
    pub(crate) fn new(
        blocked: RequestId,
    ) -> (Arc<Self>, Receiver<RequestTestEvent>, SyncSender<()>) {
        let (events, receiver) = mpsc::channel();
        let (release, wait) = mpsc::sync_channel(0);
        (
            Arc::new(Self {
                blocked,
                events,
                release: Mutex::new(wait),
            }),
            receiver,
            release,
        )
    }

    fn before_execute(&self, request: Option<&RequestId>) {
        self.wait(request, RequestTestEvent::SnapshotCaptured);
    }

    fn before_commit(&self, request: Option<&RequestId>) {
        self.wait(request, RequestTestEvent::WorkCompleted);
    }

    fn wait(&self, request: Option<&RequestId>, event: RequestTestEvent) {
        if request != Some(&self.blocked) {
            return;
        }
        let _ = self.events.send(event);
        let receiver = self
            .release
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _ = receiver.recv();
    }
}

/// The coordinator-owned request scheduler.
///
/// Workers receive only detached snapshots and return their outcome through
/// `ServerEvent`. The coordinator remains the only owner of live server state
/// and the protocol writer.
pub(crate) struct RequestScheduler {
    events: Sender<ServerEvent>,
    pending: usize,
    #[cfg(test)]
    test_barrier: Option<Arc<RequestTestBarrier>>,
}

impl RequestScheduler {
    pub(crate) fn new(
        events: Sender<ServerEvent>,
        #[cfg(test)] test_barrier: Option<Arc<RequestTestBarrier>>,
    ) -> Self {
        Self {
            events,
            pending: 0,
            #[cfg(test)]
            test_barrier,
        }
    }

    pub(crate) const fn is_idle(&self) -> bool {
        self.pending == 0
    }

    /// Spawn detached work, rejecting ambiguous duplicate active identifiers.
    pub(crate) fn schedule(
        &mut self,
        server: &Server,
        response_id: Value,
        work: RequestWork,
    ) -> io::Result<ScheduleResult> {
        let request_id = RequestId::from_json(&response_id);
        let token = match request_id.as_ref() {
            Some(request_id) => match server.cancellation.begin(request_id.clone()) {
                Some(token) => Some(token),
                None => return Ok(ScheduleResult::DuplicateIdentifier(response_id)),
            },
            None => None,
        };
        let events = self.events.clone();
        let cancellation = server.cancellation.clone();
        let worker_request_id = request_id.clone();
        let worker_token = token.clone();
        #[cfg(test)]
        let test_barrier = self.test_barrier.clone();
        let handle = thread::Builder::new()
            .name("pure-analyzer-lsp-request".to_owned())
            .spawn(move || {
                #[cfg(test)]
                if let Some(test_barrier) = &test_barrier {
                    test_barrier.before_execute(worker_request_id.as_ref());
                }
                let outcome = if worker_token
                    .as_ref()
                    .is_some_and(CancellationToken::is_cancelled)
                {
                    WorkerOutcome::Cancelled
                } else {
                    match catch_unwind(AssertUnwindSafe(|| work.execute())) {
                        Ok(completion) => WorkerOutcome::Completed(completion),
                        Err(_) => WorkerOutcome::Panicked,
                    }
                };
                #[cfg(test)]
                if let Some(test_barrier) = &test_barrier {
                    test_barrier.before_commit(worker_request_id.as_ref());
                }
                let _ = events.send(ServerEvent::Completed(CompletedRequest {
                    response_id,
                    request_id: worker_request_id,
                    token: worker_token,
                    outcome,
                }));
            });
        if let Err(error) = handle {
            if let (Some(request_id), Some(token)) = (request_id.as_ref(), token.as_ref()) {
                let _ = cancellation.finish(request_id, token);
            }
            return Err(error);
        }
        self.pending = self.pending.saturating_add(1);
        Ok(ScheduleResult::Scheduled)
    }

    /// Commit one completed worker result at the coordinator boundary.
    pub(crate) fn complete<W: Write>(
        &mut self,
        server: &Server,
        writer: &mut W,
        completed: CompletedRequest,
    ) -> io::Result<()> {
        self.pending = self.pending.saturating_sub(1);
        let cancelled = match (&completed.request_id, &completed.token) {
            (Some(request_id), Some(token)) => server.cancellation.finish(request_id, token),
            (None, None) => false,
            _ => true,
        };
        if cancelled || matches!(&completed.outcome, WorkerOutcome::Cancelled) {
            return send_error(
                writer,
                completed.response_id,
                REQUEST_CANCELLED_CODE,
                "request cancelled",
            );
        }
        match completed.outcome {
            WorkerOutcome::Completed(completion) => {
                if completion.is_current(server) {
                    send_result(writer, completed.response_id, completion.into_result())
                } else {
                    send_result(
                        writer,
                        completed.response_id,
                        completion.stale_result().clone(),
                    )
                }
            }
            WorkerOutcome::Panicked => send_error(
                writer,
                completed.response_id,
                INTERNAL_ERROR_CODE,
                "request failed",
            ),
            WorkerOutcome::Cancelled => send_error(
                writer,
                completed.response_id,
                REQUEST_CANCELLED_CODE,
                "request cancelled",
            ),
        }
    }
}

/// Whether scheduling allocated an independent worker.
#[derive(Debug)]
pub(crate) enum ScheduleResult {
    /// The worker owns its response identifier.
    Scheduled,
    /// An active request already owns the response identifier.
    DuplicateIdentifier(Value),
}

/// A worker completion awaiting coordinator validation and output.
pub(crate) struct CompletedRequest {
    response_id: Value,
    request_id: Option<RequestId>,
    token: Option<CancellationToken>,
    outcome: WorkerOutcome,
}

enum WorkerOutcome {
    Completed(RequestCompletion),
    Cancelled,
    Panicked,
}
