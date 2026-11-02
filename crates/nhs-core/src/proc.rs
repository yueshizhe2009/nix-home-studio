//! Running the two programs this application is allowed to run.
//!
//! This is the **only** module in the crate that creates a child process, and the
//! set of programs is a closed enum rather than a string: a caller cannot name a
//! system rebuild tool, a privilege-escalation helper or a shell, because no
//! value of [`Program`] describes them. That is the primary guard; the source
//! scan in `tests/safety.rs` is a backstop.
//!
//! Two properties matter more than features here:
//!
//! * **nothing is interpreted.** Arguments are passed as separate `OsString`s,
//!   never joined into a shell string, so a package name cannot become a command.
//! * **a run can always be stopped.** The child is placed in its own process
//!   group, and cancellation or a timeout signals the whole group — a `nix`
//!   build spawns builders, and killing only the direct child would leave them
//!   running and holding the output pipes open.
//!
//! `RealRunner` is the only implementation that touches the operating system.
//! `Runner` exists so tests (and later the GUI) can substitute a scripted one.

use std::ffi::OsString;
use std::io::{self, BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

#[cfg(unix)]
use std::os::unix::process::CommandExt;

use crate::env::EnvView;

/// How often the run loop checks for cancellation, timeout and exit.
const POLL: Duration = Duration::from_millis(20);

/// A program this application may run.
///
/// Deliberately an enum: the allowlist is enforced by the type system, not by a
/// string comparison that a future caller could forget.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Program {
    /// The Nix CLI, used for `build --dry-run`.
    Nix,
    /// The `home-manager` CLI, used for classic (non-flake) builds.
    HomeManager,
}

impl Program {
    /// Every program the allowlist contains.
    pub const ALL: [Self; 2] = [Self::Nix, Self::HomeManager];

    /// The file name to resolve on `PATH`.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Nix => "nix",
            Self::HomeManager => "home-manager",
        }
    }

    /// The only way a name becomes a [`Program`].
    ///
    /// Used when a command is reconstructed from persisted data, and by the
    /// tests that prove the allowlist is closed. Anything not listed is `None`.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|program| program.name() == name)
    }
}

impl std::fmt::Display for Program {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

/// Which pipe a line came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stream {
    Stdout,
    Stderr,
}

/// Everything needed to run one command.
///
/// There is no field for a program name, a shell string or an environment
/// override: arguments are structured, and the environment is inherited as-is
/// (the CLI never injects `NIX_*`).
#[derive(Debug, Clone)]
pub struct CommandSpec {
    /// Which allowlisted program to run.
    pub program: Program,
    /// Arguments, passed one by one — never joined into a shell command.
    pub args: Vec<OsString>,
    /// Working directory. Validating always runs in a scratch copy.
    pub cwd: PathBuf,
    /// Wall-clock limit for the whole run.
    pub timeout: Duration,
    /// Grace period between `SIGTERM` and `SIGKILL` when stopping the group.
    pub kill_grace: Duration,
    /// Retain at most this many bytes of each stream in [`Outcome`].
    ///
    /// `None` keeps everything, which is what the diagnostic path wants: the
    /// classifier reads the *end* of a build log, so truncating there would lose
    /// the error. A caller that streams a large catalogue sets a bound, because
    /// there the interesting output is consumed line by line and the retained
    /// copy only needs to be bounded, not complete.
    ///
    /// The limit applies to retention only: every line still reaches the sink,
    /// and the child is never blocked by it.
    pub max_capture_bytes: Option<u64>,
}

impl CommandSpec {
    /// A command with a five-minute timeout and a two-second kill grace.
    #[must_use]
    pub fn new(program: Program, cwd: impl Into<PathBuf>) -> Self {
        Self {
            program,
            args: Vec::new(),
            cwd: cwd.into(),
            timeout: Duration::from_secs(300),
            kill_grace: Duration::from_secs(2),
            max_capture_bytes: None,
        }
    }

    /// Append one argument.
    #[must_use]
    pub fn arg(mut self, arg: impl Into<OsString>) -> Self {
        self.args.push(arg.into());
        self
    }

    /// Append several arguments.
    #[must_use]
    pub fn args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<OsString>,
    {
        self.args.extend(args.into_iter().map(Into::into));
        self
    }

    /// Replace the timeout.
    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Replace the kill grace.
    #[must_use]
    pub fn with_kill_grace(mut self, grace: Duration) -> Self {
        self.kill_grace = grace;
        self
    }

    /// Bound the retained copy of each stream. Streaming is unaffected.
    #[must_use]
    pub fn with_max_capture(mut self, bytes: u64) -> Self {
        self.max_capture_bytes = Some(bytes);
        self
    }

    /// The command line as it would be printed to the user.
    ///
    /// Only for display and for "would run" messages; execution never goes
    /// through a string.
    #[must_use]
    pub fn display(&self) -> String {
        let mut out = self.program.name().to_string();
        for arg in &self.args {
            out.push(' ');
            out.push_str(&arg.to_string_lossy());
        }
        out
    }
}

/// A flag one thread can set to stop a run.
#[derive(Debug, Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Ask the run to stop. Idempotent, and safe from any thread.
    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    /// Clear the flag so the same session can run again.
    ///
    /// A cancelled run must not poison the next one: the front end reuses one
    /// session for the whole session, and the worker resets the token before it
    /// starts a job. Safe from any thread.
    pub fn reset(&self) {
        self.0.store(false, Ordering::SeqCst);
    }

    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

/// How a finished run ended.
#[derive(Debug, Clone)]
pub struct Outcome {
    pub status: ExitStatus,
    /// True when a [`CancelToken`] stopped it.
    pub cancelled: bool,
    /// True when the timeout stopped it.
    pub timed_out: bool,
    pub stdout: String,
    pub stderr: String,
    /// True when a stream was longer than [`CommandSpec::max_capture_bytes`] and
    /// the retained copy stops at the limit. The sink saw every line regardless.
    pub truncated: bool,
}

impl Outcome {
    /// True when the program exited successfully and nothing stopped it early.
    #[must_use]
    pub fn success(&self) -> bool {
        self.status.success() && !self.cancelled && !self.timed_out
    }
}

/// Why a run could not be started or could not be observed.
#[derive(Debug)]
pub enum RunError {
    /// The program is not on the configured `PATH`.
    NotFound { program: Program },
    /// The resolved file name does not match the allowlisted program.
    NotAllowlisted { program: Program, resolved: PathBuf },
    /// Creating the process failed.
    Spawn { program: Program, source: io::Error },
    /// Waiting for the process failed.
    Wait { program: Program, source: io::Error },
}

impl std::fmt::Display for RunError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound { program } => write!(
                f,
                "`{program}` was not found on PATH; install it or point this application at it"
            ),
            Self::NotAllowlisted { program, resolved } => write!(
                f,
                "`{program}` resolved to {}, which is not the allowlisted executable",
                resolved.display()
            ),
            Self::Spawn { program, source } => write!(f, "could not start `{program}`: {source}"),
            Self::Wait { program, source } => {
                write!(f, "could not wait for `{program}` to finish: {source}")
            }
        }
    }
}

impl std::error::Error for RunError {}

/// Something that can run a [`CommandSpec`].
///
/// The CLI and the validator depend on this trait, never on `RealRunner`, so a
/// test can record argv and script outcomes without starting a process.
pub trait Runner {
    /// Run `spec`, forwarding every output line to `sink` as it arrives.
    fn run(
        &self,
        spec: &CommandSpec,
        cancel: &CancelToken,
        sink: &mut dyn FnMut(Stream, &str),
    ) -> Result<Outcome, RunError>;
}

/// The real implementation: resolves the program from [`EnvView`] and spawns it.
pub struct RealRunner<'a> {
    env: &'a EnvView,
}

impl<'a> RealRunner<'a> {
    #[must_use]
    pub fn new(env: &'a EnvView) -> Self {
        Self { env }
    }

    /// Resolve the allowlisted program to an absolute path.
    ///
    /// The resolved file name must still be the allowlisted name: a `PATH` entry
    /// that somehow yields a different file is refused rather than executed.
    fn resolve(&self, program: Program) -> Result<PathBuf, RunError> {
        let resolved = self
            .env
            .find_on_path(program.name())
            .ok_or(RunError::NotFound { program })?;
        let file_name = resolved.file_name().and_then(|name| name.to_str());
        if file_name != Some(program.name()) {
            return Err(RunError::NotAllowlisted { program, resolved });
        }
        Ok(resolved)
    }
}

impl Runner for RealRunner<'_> {
    fn run(
        &self,
        spec: &CommandSpec,
        cancel: &CancelToken,
        sink: &mut dyn FnMut(Stream, &str),
    ) -> Result<Outcome, RunError> {
        let executable = self.resolve(spec.program)?;
        let mut command = Command::new(&executable);
        command
            .args(&spec.args)
            .current_dir(&spec.cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        // Its own process group, so cancelling reaches the builders `nix` starts.
        #[cfg(unix)]
        command.process_group(0);

        let mut child = command.spawn().map_err(|source| RunError::Spawn {
            program: spec.program,
            source,
        })?;
        let pid = child.id();
        let (sender, receiver) = mpsc::channel::<(Stream, String)>();
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        if let Some(pipe) = stdout {
            forward_lines(pipe, Stream::Stdout, sender.clone());
        }
        if let Some(pipe) = stderr {
            forward_lines(pipe, Stream::Stderr, sender.clone());
        }
        drop(sender);

        let started = Instant::now();
        let mut out = Collected::with_limit(spec.max_capture_bytes);
        let mut stopped_at: Option<Instant> = None;
        let mut cancelled = false;
        let mut timed_out = false;

        loop {
            match receiver.recv_timeout(POLL) {
                Ok((stream, line)) => {
                    sink(stream, &line);
                    out.push(stream, &line);
                    continue;
                }
                Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => {}
            }

            if let Some(status) = child.try_wait().map_err(|source| RunError::Wait {
                program: spec.program,
                source,
            })? {
                // Drain anything the reader threads buffered before exiting.
                while let Ok((stream, line)) = receiver.try_recv() {
                    sink(stream, &line);
                    out.push(stream, &line);
                }
                return Ok(Outcome {
                    status,
                    cancelled,
                    timed_out,
                    stdout: out.stdout,
                    stderr: out.stderr,
                    truncated: out.truncated,
                });
            }

            if stopped_at.is_none() {
                if cancel.is_cancelled() {
                    cancelled = true;
                    stopped_at = Some(Instant::now());
                    signal_group(pid, libc::SIGTERM);
                } else if started.elapsed() >= spec.timeout {
                    timed_out = true;
                    stopped_at = Some(Instant::now());
                    signal_group(pid, libc::SIGTERM);
                }
            } else if stopped_at.is_some_and(|at| at.elapsed() >= spec.kill_grace) {
                // It ignored SIGTERM; take the whole group down.
                signal_group(pid, libc::SIGKILL);
            }
        }
    }
}

/// Forward a pipe's lines to the run loop as they arrive.
///
/// A separate thread per pipe is what makes output streaming rather than a
/// capture of everything at exit; it also stops a chatty child from filling a
/// full pipe buffer while we are still waiting for it to exit.
fn forward_lines<R>(pipe: R, stream: Stream, sender: mpsc::Sender<(Stream, String)>)
where
    R: io::Read + Send + 'static,
{
    thread::spawn(move || {
        for line in BufReader::new(pipe).lines() {
            let Ok(line) = line else { break };
            if sender.send((stream, line)).is_err() {
                break;
            }
        }
    });
}

/// Collected output, kept as lines and bounded on request.
#[derive(Default)]
struct Collected {
    stdout: String,
    stderr: String,
    truncated: bool,
    limit: Option<u64>,
}

impl Collected {
    fn with_limit(limit: Option<u64>) -> Self {
        Self {
            limit,
            ..Self::default()
        }
    }

    fn push(&mut self, stream: Stream, line: &str) {
        if let Some(limit) = self.limit {
            let used = match stream {
                Stream::Stdout => self.stdout.len(),
                Stream::Stderr => self.stderr.len(),
            };
            let needed = u64::try_from(used)
                .unwrap_or(u64::MAX)
                .saturating_add(u64::try_from(line.len()).unwrap_or(u64::MAX))
                .saturating_add(1);
            if needed > limit {
                self.truncated = true;
                return;
            }
        }
        let target = match stream {
            Stream::Stdout => &mut self.stdout,
            Stream::Stderr => &mut self.stderr,
        };
        if !target.is_empty() {
            target.push('\n');
        }
        target.push_str(line);
    }
}

/// Signal the process group led by `pid`.
///
/// The child was spawned with `process_group(0)`, so its group id equals its pid
/// and `-pid` addresses the whole group: the program plus every builder it
/// started. Signalling a group that has already exited is a harmless `ESRCH`.
fn signal_group(pid: u32, signal: i32) {
    let Ok(group) = i32::try_from(pid) else {
        return;
    };
    // SAFETY: `kill(2)` is called with a negative pid, which addresses the
    // process group whose id is that absolute value. The group was created by
    // this process via `process_group(0)`, so it does not belong to another
    // user's session, and `signal` is one of the two constants passed by this
    // module (`SIGTERM`/`SIGKILL`). The return value is ignored on purpose: a
    // group that has already exited yields `ESRCH`, which needs no handling.
    unsafe {
        libc::kill(-group, signal);
    }
}

/// True when the process is still alive (used by tests and diagnostics).
#[must_use]
pub fn is_running(pid: u32) -> bool {
    Path::new(&format!("/proc/{pid}")).exists()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_allowlist_is_closed() {
        assert_eq!(Program::ALL.len(), 2);
        assert_eq!(Program::from_name("nix"), Some(Program::Nix));
        assert_eq!(
            Program::from_name("home-manager"),
            Some(Program::HomeManager)
        );
        // The privilege-escalating and shell names are asserted in
        // `tests/proc.rs`: this file is scanned by gate G3, and the names this
        // application must never run have no business appearing in `src/`.
        for forbidden in ["", "nix ", "NIX", "./nix", "home-manager "] {
            assert_eq!(
                Program::from_name(forbidden),
                None,
                "`{forbidden}` must not be expressible as a Program"
            );
        }
    }

    #[test]
    fn the_displayed_command_is_one_line_and_unquoted_by_a_shell() {
        let spec = CommandSpec::new(Program::Nix, "/tmp").args([
            "build",
            "--dry-run",
            ".#homeConfigurations.me.activationPackage",
        ]);
        assert_eq!(
            spec.display(),
            "nix build --dry-run .#homeConfigurations.me.activationPackage"
        );
    }
}
