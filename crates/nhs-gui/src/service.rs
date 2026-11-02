//! The only place the GUI touches the world.
//!
//! One mutex guards the state machine plus the "which job is running" slot. It is
//! **never held while a process runs**: the worker reads its parameters under the
//! lock, releases it, does the work, and only takes it again to stream log lines,
//! feed the result through `nhs-app`, and clear the slot. That is what lets the
//! window stay responsive without letting two jobs race.
//!
//! The safety gate is not duplicated here. `nhs-app` decides whether an apply is
//! allowed; the worker only performs the intent it was handed, and the result goes
//! back through `App::handle`, so revision/attempt checks apply to asynchronous
//! work exactly as they do to synchronous work.

use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use nhs_adapter::Session;
use nhs_app::{App, ApplyResult, Event, Intent};
use nhs_core::proc::Stream;

use crate::bridge::{state_of, BridgeError, BusyInfo, CancelAck, UiState};

/// How the shell is told that the state changed (Tauri `emit`, a channel in tests).
type Notifier = Arc<dyn Fn(&UiState) + Send + Sync>;

/// Starts a background job. Injectable so a test can make it fail.
type Spawner = Arc<dyn Fn(Box<dyn FnOnce() + Send>) -> io::Result<()> + Send + Sync>;

/// Log lines are pushed to the window in batches, not one by one: a build can
/// print thousands of lines and every push re-renders the tree.
///
/// A ticker flushes on its own clock. Flushing only when a line arrives would
/// hold the last line of a quiet build until the process exits, which is exactly
/// the case a user stares at.
const LOG_FLUSH: Duration = Duration::from_millis(100);
const LOG_TICK: Duration = Duration::from_millis(25);
const LOG_BATCH: usize = 50;

pub struct Service {
    inner: Arc<Inner>,
}

struct Inner {
    state: Mutex<Core>,
    session: Session,
    notify: Mutex<Option<Notifier>>,
    spawn: Spawner,
}

struct Core {
    app: App,
    running: Option<Running>,
    next_id: u64,
}

#[derive(Clone, Copy)]
struct Running {
    id: u64,
    kind: &'static str,
    revision: u64,
    attempt: u64,
}

impl Service {
    #[must_use]
    pub fn new(session: Session) -> Self {
        Self::with_spawner(
            session,
            Arc::new(|task| {
                thread::Builder::new()
                    .name("nhs-worker".to_string())
                    .spawn(task)
                    .map(|_| ())
            }),
        )
    }

    /// A service with a custom spawner, so the "worker could not start" path can
    /// be exercised deterministically.
    #[must_use]
    pub fn with_spawner(session: Session, spawn: Spawner) -> Self {
        Self {
            inner: Arc::new(Inner {
                state: Mutex::new(Core {
                    app: App::new(),
                    running: None,
                    next_id: 0,
                }),
                session,
                notify: Mutex::new(None),
                spawn,
            }),
        }
    }

    /// Called once by the shell; sends every state change to the web view.
    ///
    /// # Errors
    /// `internal` when the notifier slot is poisoned.
    pub fn set_notifier(&self, notifier: Notifier) -> Result<(), BridgeError> {
        let mut slot = self
            .inner
            .notify
            .lock()
            .map_err(|_| BridgeError::internal("the notification slot is poisoned"))?;
        *slot = Some(notifier);
        Ok(())
    }

    /// Read-only snapshot.
    ///
    /// # Errors
    /// `internal` when the state lock is poisoned.
    pub fn snapshot(&self) -> Result<UiState, BridgeError> {
        let core = self.lock()?;
        Ok(Self::state_of(&core))
    }

    /// The single state-changing entry point.
    ///
    /// # Errors
    /// `busy` while a background job runs, `internal` on a poisoned lock.
    pub fn dispatch(&self, event: Event) -> Result<UiState, BridgeError> {
        let mut core = self.lock()?;
        if let Some(running) = core.running {
            return Err(BridgeError::busy(running.kind));
        }

        let mut response = core.app.handle(event);
        let refusal = response.refusal;
        let mut job: Option<(Running, Intent)> = None;
        let mut rounds = 0_u8;
        while let Some(intent) = response.intents.first().cloned() {
            if rounds >= 4 {
                break;
            }
            rounds = rounds.saturating_add(1);
            match intent {
                Intent::Discover => response = core.app.handle(self.inner.session.discover()),
                Intent::Search {
                    query,
                    filters,
                    page,
                    per_page,
                } => {
                    response = core
                        .app
                        .handle(self.inner.session.search(&query, &filters, page, per_page));
                }
                Intent::BuildPlan { revision } => {
                    let selection = nhs_core::flow::Selection {
                        packages: core.app.selected().to_vec(),
                        settings: core.app.settings().to_vec(),
                    };
                    response = core
                        .app
                        .handle(self.inner.session.plan(&selection, revision));
                }
                Intent::LoadHistory => response = core.app.handle(self.inner.session.history()),
                Intent::Validate { revision, attempt } => {
                    // Reset before the slot becomes visible: a cancel that arrives
                    // right after the job is registered must not be cleared by it.
                    self.inner.session.reset_cancel();
                    job = Some((
                        core.register("validation", revision, attempt),
                        Intent::Validate { revision, attempt },
                    ));
                    break;
                }
                Intent::Apply { revision } => {
                    self.inner.session.reset_cancel();
                    job = Some((
                        core.register("apply", revision, 0),
                        Intent::Apply { revision },
                    ));
                    break;
                }
            }
        }

        let snapshot = Self::state_of(&core);
        drop(core);

        if let Some((running, intent)) = job {
            self.start(running, intent);
        }
        if let Some(refusal) = refusal {
            return Err(BridgeError::refused(refusal.code.code()));
        }
        Ok(snapshot)
    }

    /// Cancel the job that is running *now*. Idle is an error, not a no-op that
    /// pretends to have stopped something, and an apply is refused because the
    /// write path does not read the token: reporting success there would be a lie.
    ///
    /// # Errors
    /// `cancel_unavailable` when nothing is running or the job cannot be
    /// interrupted, `internal` on a poisoned lock.
    pub fn cancel(&self) -> Result<CancelAck, BridgeError> {
        let core = self.lock()?;
        let Some(running) = core.running else {
            return Err(BridgeError::cancel_unavailable());
        };
        if !job_is_cancellable(running.kind) {
            return Err(BridgeError::cancel_unsupported(running.kind));
        }
        self.inner.session.cancel();
        Ok(CancelAck { job: running.id })
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, Core>, BridgeError> {
        self.inner
            .state
            .lock()
            .map_err(|_| BridgeError::internal("the interface state lock is poisoned"))
    }

    fn state_of(core: &Core) -> UiState {
        let mut state = state_of(&core.app);
        state.busy = core.running.map(|running| BusyInfo {
            kind: running.kind,
            revision: running.revision,
            attempt: running.attempt,
        });
        state
    }

    /// Run the intent on a worker thread, leaving the lock free for the window.
    fn start(&self, running: Running, intent: Intent) {
        let inner = Arc::clone(&self.inner);
        let spawn = Arc::clone(&inner.spawn);
        let spawned = spawn(Box::new(move || {
            let buffer = Arc::new(LogBuffer::new());
            let ticker = spawn_ticker(Arc::clone(&inner), Arc::clone(&buffer));
            let outcome = inner.run(running, intent, &buffer);
            buffer.stop();
            if let Some(handle) = ticker {
                let _ = handle.join();
            }
            inner.flush(buffer.take());
            inner.finish(running, outcome);
        }));
        if spawned.is_err() {
            self.inner.start_failed(running);
        }
    }
}

impl Inner {
    // `intent` is matched by value because the closure owns it; the lint only sees
    // that every arm borrows, which is fine here.
    #[allow(clippy::needless_pass_by_value)]
    fn run(&self, _running: Running, intent: Intent, buffer: &LogBuffer) -> Event {
        match intent {
            Intent::Validate { revision, attempt } => {
                let mut sink = |stream: Stream, text: &str| {
                    buffer.push(stream, text);
                    if buffer.ready() {
                        self.flush(buffer.take());
                    }
                };
                self.session.validate(revision, attempt, &mut sink)
            }
            Intent::Apply { revision } => self.session.apply(revision),
            _ => Event::Progress("the worker was handed an intent it cannot run".to_string()),
        }
    }

    /// Push one batch of log lines through the state machine and tell the shell.
    fn flush(&self, lines: Vec<(Stream, String)>) {
        if lines.is_empty() {
            return;
        }
        if let Ok(mut core) = self.state.lock() {
            for (stream, text) in lines {
                let logged = match stream {
                    Stream::Stdout => nhs_app::LogStream::Stdout,
                    Stream::Stderr => nhs_app::LogStream::Stderr,
                };
                core.app.handle(Event::LogLine {
                    stream: logged,
                    text,
                });
            }
        }
        self.emit_state();
    }

    /// Feed the worker's result through the state machine and release the slot.
    fn finish(&self, running: Running, event: Event) {
        if let Ok(mut core) = self.state.lock() {
            // Only the current job may report; a late result from an older job is
            // dropped here and, independently, by the revision checks in `nhs-app`.
            if core.running.map(|current| current.id) == Some(running.id) {
                core.app.handle(event);
                core.running = None;
            }
        }
        self.emit_state();
    }

    /// The worker could not be created. Clear the slot and leave a state the user
    /// can act on — never a job that looks busy forever.
    fn start_failed(&self, running: Running) {
        if let Ok(mut core) = self.state.lock() {
            if core.running.map(|current| current.id) == Some(running.id) {
                if running.kind == "apply" {
                    core.app.handle(Event::ApplyFinished {
                        revision: running.revision,
                        result: ApplyResult::Failed {
                            message: "the background writer could not start; nothing was written"
                                .to_string(),
                        },
                    });
                } else {
                    core.app.handle(Event::Progress(
                        "the background worker could not start; nothing was changed".to_string(),
                    ));
                }
                core.running = None;
            }
        }
        self.emit_state();
    }

    fn emit_state(&self) {
        let state = match self.state.lock() {
            Ok(core) => {
                let mut state = state_of(&core.app);
                state.busy = core.running.map(|running| BusyInfo {
                    kind: running.kind,
                    revision: running.revision,
                    attempt: running.attempt,
                });
                state
            }
            Err(_) => return,
        };
        let notifier = self
            .notify
            .lock()
            .ok()
            .and_then(|slot| slot.as_ref().map(Arc::clone));
        if let Some(notify) = notifier {
            notify(&state);
        }
    }
}

impl Core {
    fn register(&mut self, kind: &'static str, revision: u64, attempt: u64) -> Running {
        self.next_id = self.next_id.saturating_add(1);
        let running = Running {
            id: self.next_id,
            kind,
            revision,
            attempt,
        };
        self.running = Some(running);
        running
    }
}

/// Applies write through `EditEngine::commit` and never read the token, so
/// offering cancellation for them would be dishonest.
#[must_use]
pub fn job_is_cancellable(kind: &str) -> bool {
    kind != "apply"
}

/// Bounded buffer for streamed log lines, safe to touch from the sink and from
/// the ticker at the same time.
struct LogBuffer {
    pending: Mutex<Pending>,
    stopped: AtomicBool,
}

struct Pending {
    lines: Vec<(Stream, String)>,
    last: Instant,
}

impl LogBuffer {
    fn new() -> Self {
        Self {
            pending: Mutex::new(Pending {
                lines: Vec::new(),
                last: Instant::now(),
            }),
            stopped: AtomicBool::new(false),
        }
    }

    fn push(&self, stream: Stream, text: &str) {
        if let Ok(mut pending) = self.pending.lock() {
            pending.lines.push((stream, text.to_string()));
        }
    }

    fn ready(&self) -> bool {
        self.pending.lock().is_ok_and(|pending| {
            pending.lines.len() >= LOG_BATCH || pending.last.elapsed() >= LOG_FLUSH
        })
    }

    fn take(&self) -> Vec<(Stream, String)> {
        match self.pending.lock() {
            Ok(mut pending) => {
                pending.last = Instant::now();
                std::mem::take(&mut pending.lines)
            }
            Err(_) => Vec::new(),
        }
    }

    fn stop(&self) {
        self.stopped.store(true, Ordering::SeqCst);
    }

    fn stopped(&self) -> bool {
        self.stopped.load(Ordering::SeqCst)
    }
}

/// Flush on a fixed clock so a silent build still reaches the window.
fn spawn_ticker(inner: Arc<Inner>, buffer: Arc<LogBuffer>) -> Option<thread::JoinHandle<()>> {
    thread::Builder::new()
        .name("nhs-log-tick".to_string())
        .spawn(move || loop {
            thread::sleep(LOG_TICK);
            if buffer.stopped() {
                break;
            }
            if buffer.ready() {
                let lines = buffer.take();
                inner.flush(lines);
            }
        })
        .ok()
}
