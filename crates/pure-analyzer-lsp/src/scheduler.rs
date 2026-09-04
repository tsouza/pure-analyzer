//! Front-end-only scheduling for cancellable LSP requests.
//!
//! Requests execute on the shared `rayon` global thread pool rather than one
//! dedicated OS thread per request (see `RequestScheduler::schedule`): the
//! pool bounds live worker threads to its (CPU-count-sized, by default)
//! configuration regardless of how many requests a client has in flight,
//! queuing the rest in memory instead of asking the OS for an unbounded
//! number of threads.

use std::{
    io::{self, Write},
    panic::{AssertUnwindSafe, catch_unwind},
    sync::mpsc::Sender,
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

    /// Queue detached work onto the shared pool, rejecting ambiguous
    /// duplicate active identifiers.
    ///
    /// Unlike a dedicated `thread::spawn` per request, queuing onto `rayon`'s
    /// bounded pool cannot fail the way OS thread creation can once a
    /// client's in-flight request count grows far past what the host can
    /// spawn threads for (see the crate-level scheduling note); scheduling is
    /// therefore infallible.
    pub(crate) fn schedule(
        &mut self,
        server: &Server,
        response_id: Value,
        work: RequestWork,
    ) -> ScheduleResult {
        let request_id = RequestId::from_json(&response_id);
        let token = match request_id.as_ref() {
            Some(request_id) => match server.cancellation.begin(request_id.clone()) {
                Some(token) => Some(token),
                None => return ScheduleResult::DuplicateIdentifier(response_id),
            },
            None => None,
        };
        let events = self.events.clone();
        let worker_request_id = request_id.clone();
        let worker_token = token.clone();
        #[cfg(test)]
        let test_barrier = self.test_barrier.clone();
        rayon::spawn(move || {
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
        self.pending = self.pending.saturating_add(1);
        ScheduleResult::Scheduled
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

/// Whether scheduling queued an independent worker.
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

#[cfg(test)]
mod tests {
    use std::{
        io::Cursor,
        sync::{
            Arc, Condvar, Mutex, PoisonError,
            atomic::{AtomicUsize, Ordering},
            mpsc,
        },
        thread,
        time::Duration,
    };

    use serde_json::Value;

    use super::{
        CompletedRequest, INTERNAL_ERROR_CODE, REQUEST_CANCELLED_CODE, RequestScheduler,
        ScheduleResult, WorkerOutcome,
    };
    use crate::{RequestId, Server, frame::read_frame, server::ServerEvent, state};

    fn idle_scheduler() -> RequestScheduler {
        let (events, _receiver) = mpsc::channel();
        RequestScheduler::new(events, None)
    }

    fn only_frame(output: &[u8]) -> Value {
        let mut reader = Cursor::new(output);
        read_frame(&mut reader)
            .expect("frame parses")
            .expect("one frame was written")
    }

    #[test]
    fn complete_reports_internal_error_for_a_panicked_worker_without_a_cancellable_identifier() {
        let server = Server::new();
        let mut scheduler = idle_scheduler();
        let mut output = Vec::new();
        let completed = CompletedRequest {
            response_id: Value::Number(1.into()),
            request_id: None,
            token: None,
            outcome: WorkerOutcome::Panicked,
        };

        scheduler
            .complete(&server, &mut output, completed)
            .expect("writing the error frame succeeds");

        let frame = only_frame(&output);
        assert_eq!(frame["error"]["code"], INTERNAL_ERROR_CODE);
        assert_eq!(frame["error"]["message"], "request failed");
    }

    #[test]
    fn complete_reports_cancellation_ahead_of_a_panic_once_the_registry_cancelled_the_request() {
        let server = Server::new();
        let id = RequestId::Number(7);
        let token = server
            .cancellation
            .begin(id.clone())
            .expect("registry accepts a fresh id");
        server.cancellation.cancel(id.clone());
        let mut scheduler = idle_scheduler();
        let mut output = Vec::new();
        let completed = CompletedRequest {
            response_id: Value::Number(2.into()),
            request_id: Some(id),
            token: Some(token),
            outcome: WorkerOutcome::Panicked,
        };

        scheduler
            .complete(&server, &mut output, completed)
            .expect("writing the error frame succeeds");

        let frame = only_frame(&output);
        assert_eq!(frame["error"]["code"], REQUEST_CANCELLED_CODE);
        assert_eq!(frame["error"]["message"], "request cancelled");
        assert!(
            server.cancellation.is_empty(),
            "the coordinator must finish (and remove) the entry even for a panicked worker"
        );
    }

    #[test]
    fn complete_reports_cancellation_from_the_worker_outcome_without_a_registry_entry() {
        let server = Server::new();
        let mut scheduler = idle_scheduler();
        let mut output = Vec::new();
        let completed = CompletedRequest {
            response_id: Value::Number(3.into()),
            request_id: None,
            token: None,
            outcome: WorkerOutcome::Cancelled,
        };

        scheduler
            .complete(&server, &mut output, completed)
            .expect("writing the error frame succeeds");

        let frame = only_frame(&output);
        assert_eq!(frame["error"]["code"], REQUEST_CANCELLED_CODE);
        assert_eq!(frame["error"]["message"], "request cancelled");
    }

    /// Regression guard for the property `RequestScheduler::schedule` now
    /// relies on instead of one OS thread per request (issue #429): `rayon`
    /// bounds truly concurrent execution to its pool size and queues the
    /// rest, rather than growing OS thread count with the job count.
    ///
    /// A prior version spawned a raw `thread::Builder` per request; a local
    /// load test proved that scales to only tens of thousands of
    /// concurrently *blocked* requests before `thread::Builder::spawn` starts
    /// failing outright (observed failure around ~38k concurrent threads on
    /// an 8-core/31GiB workstation, well within a chatty or buggy client's
    /// reach once requests take any nonzero time), and that every such
    /// failure previously terminated the whole `Server::serve` session. This
    /// test holds `4 * rayon::current_num_threads()` jobs blocked at once —
    /// deliberately more jobs than the pool has threads — and asserts peak
    /// concurrent execution never exceeds the pool size, which is the
    /// mechanism that removes that failure mode.
    #[test]
    fn rayon_spawn_bounds_concurrent_execution_to_the_pool_size_not_the_job_count() {
        let pool_size = rayon::current_num_threads();
        let jobs = pool_size.saturating_mul(4).max(4);
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let alive = Arc::new(AtomicUsize::new(0));
        let peak_alive = Arc::new(AtomicUsize::new(0));
        let finished = Arc::new((Mutex::new(0usize), Condvar::new()));

        for _ in 0..jobs {
            let gate = gate.clone();
            let alive = alive.clone();
            let peak_alive = peak_alive.clone();
            let finished = finished.clone();
            rayon::spawn(move || {
                let now = alive.fetch_add(1, Ordering::SeqCst) + 1;
                peak_alive.fetch_max(now, Ordering::SeqCst);
                let (lock, condvar) = &*gate;
                let mut released = lock.lock().unwrap_or_else(PoisonError::into_inner);
                while !*released {
                    released = condvar
                        .wait(released)
                        .unwrap_or_else(PoisonError::into_inner);
                }
                drop(released);
                alive.fetch_sub(1, Ordering::SeqCst);
                let (count, done) = &*finished;
                let mut count = count.lock().unwrap_or_else(PoisonError::into_inner);
                *count += 1;
                done.notify_all();
            });
        }

        // Give the pool a moment to pick up as many jobs as it can run at
        // once before releasing them, so `peak_alive` reflects genuine
        // concurrency rather than a lucky single sample.
        thread::sleep(Duration::from_millis(200));
        {
            let (lock, condvar) = &*gate;
            let mut released = lock.lock().unwrap_or_else(PoisonError::into_inner);
            *released = true;
            condvar.notify_all();
        }

        let (count, done) = &*finished;
        let mut count = count.lock().unwrap_or_else(PoisonError::into_inner);
        while *count < jobs {
            let (guard, timeout) = done
                .wait_timeout(count, Duration::from_secs(30))
                .unwrap_or_else(PoisonError::into_inner);
            count = guard;
            assert!(!timeout.timed_out(), "jobs must all finish once released");
        }

        assert!(
            peak_alive.load(Ordering::SeqCst) <= pool_size,
            "peak concurrent execution ({}) must never exceed the pool size ({pool_size}) \
             even though {jobs} jobs were queued at once",
            peak_alive.load(Ordering::SeqCst)
        );
    }

    /// End-to-end regression guard: `RequestScheduler::schedule` itself
    /// accepts many more concurrent cancellable requests than any realistic
    /// LSP client would generate, and every one of them completes, with
    /// scheduling now infallible (no `thread::Builder::spawn` in the path to
    /// fail once the caller's in-flight count grows large).
    #[test]
    fn schedule_accepts_and_completes_many_concurrent_requests() {
        let requests = 500;
        let server = Server::new();
        let (events, receiver) = mpsc::channel();
        let mut scheduler = RequestScheduler::new(events, None);

        for i in 0..requests {
            let params: Value = serde_json::from_str(&format!(
                r#"{{"textDocument":{{"uri":"untitled:load-{i}"}},"position":{{"line":0,"character":0}}}}"#
            ))
            .expect("valid params JSON");
            let work = state::hover_work(&server, Some(&params)).expect("valid hover params");
            let id = Value::Number(i64::from(i).into());
            assert!(matches!(
                scheduler.schedule(&server, id, work),
                ScheduleResult::Scheduled
            ));
        }

        let mut completed = 0;
        while completed < requests {
            let ServerEvent::Completed(_) = receiver
                .recv_timeout(Duration::from_secs(30))
                .expect("every scheduled request must complete within the timeout")
            else {
                panic!("directly-scheduled requests only ever produce Completed events");
            };
            completed += 1;
        }
    }
}
