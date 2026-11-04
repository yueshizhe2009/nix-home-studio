//! The adapter: the only place a front end touches the world.
//!
//! `nhs-app` decides *what* should happen; this crate does it by calling
//! `nhs-core`, and hands the result back as an [`Event`]. Both prototype UIs
//! (M6.2) drive exactly this, so the comparison between them measures rendering
//! and interaction, not two different sets of business rules.
//!
//! The steps are synchronous on purpose: a GUI runs them on a worker thread and
//! feeds the returned event into the state machine. Nothing here prints, and
//! nothing here decides policy — every refusal comes from core as an error the
//! app already knows how to display.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use nhs_app::{ApplyResult, Event, GenerationSummary, PackageHit, SnapshotSummary};
use nhs_core::catalog::{self, Catalog, Filters, HostPlatform};
use nhs_core::discovery::detect;
use nhs_core::edit::{snapshot::SnapshotStore, EditEngine};
use nhs_core::flow::{self, Prepared, Selection};
use nhs_core::proc::{CancelToken, RealRunner, Stream};
use nhs_core::validation;
use nhs_core::{EnvView, FsView, RealFs, StateLayout};

/// What the five flows need from the machine.
pub struct Session {
    env: EnvView,
    state_dir: PathBuf,
    /// The plan behind the current revision, kept between `plan` and `validate`.
    prepared: Mutex<Option<Prepared>>,
    cancel: CancelToken,
}

impl Session {
    /// `home` overrides `$HOME`; `state_dir` is the final state directory.
    ///
    /// Everything else still comes from the process environment, including
    /// `XDG_CONFIG_HOME` and `XDG_STATE_HOME`. A caller that needs isolation
    /// (tests, the headless flow check) should build an [`EnvView`] and use
    /// [`Self::with_env`] instead of overriding only the home here.
    #[must_use]
    pub fn new(home: Option<&Path>, state_dir: Option<&Path>) -> Self {
        let mut env = EnvView::from_process();
        if let Some(home) = home {
            env.home = home.to_path_buf();
        }
        let state_dir = state_dir.map_or_else(
            || env.state_home().join("nix-home-studio"),
            Path::to_path_buf,
        );
        Self {
            env,
            state_dir,
            prepared: Mutex::new(None),
            cancel: CancelToken::new(),
        }
    }

    /// Build a session from an explicit environment.
    ///
    /// A test uses this to point `PATH` at a fake `home-manager` without touching
    /// the process environment (the rule from audit R13).
    #[must_use]
    pub fn with_env(env: EnvView, state_dir: PathBuf) -> Self {
        Self {
            env,
            state_dir,
            prepared: Mutex::new(None),
            cancel: CancelToken::new(),
        }
    }

    /// The state directory in use, for the window title or a diagnostics line.
    #[must_use]
    pub fn state_dir(&self) -> &Path {
        &self.state_dir
    }

    /// Stop a build that is in flight.
    pub fn cancel(&self) {
        self.cancel.cancel();
    }

    /// Clear the cancellation flag before a new job starts.
    ///
    /// Without this a cancelled run would make every later run abort instantly,
    /// because the flag lives as long as the session does.
    pub fn reset_cancel(&self) {
        self.cancel.reset();
    }

    /// Read the layout, the toolchain is not needed here.
    #[must_use]
    pub fn discover(&self) -> Event {
        Event::Discovered(detect(&RealFs, &self.env))
    }

    /// Search the curated catalogue. Offline, so it never fails on the network.
    #[must_use]
    pub fn search(&self, query: &str, filters: &Filters, page: usize, per_page: usize) -> Event {
        let Ok(catalog) = Catalog::builtin() else {
            // The catalogue is compiled in; a failure here is a build bug.
            return Event::Progress("the built-in catalogue is broken".to_string());
        };
        let host = HostPlatform::from_os_arch(std::env::consts::OS, std::env::consts::ARCH);
        let page = catalog::search(&catalog, query, filters, &host, page, per_page);
        Event::SearchResults {
            hits: page.hits.iter().map(hit).collect(),
            total: page.total,
            page: page.page,
            pages: page.pages,
        }
    }

    /// Build the plan for a selection, and remember it for validate and apply.
    #[must_use]
    pub fn plan(&self, selection: &Selection, revision: u64) -> Event {
        let discovery = detect(&RealFs, &self.env);
        match flow::prepare(&RealFs, &discovery.layout, &self.state_dir, None, selection) {
            Ok(prepared) => {
                let plan = prepared.plan.clone();
                match self.prepared.lock() {
                    Ok(mut slot) => *slot = Some(prepared),
                    Err(_) => {
                        return Event::PlanFailed {
                            revision,
                            message: "the adapter's plan slot is poisoned".to_string(),
                        }
                    }
                }
                Event::PlanBuilt { revision, plan }
            }
            Err(error) => Event::PlanFailed {
                revision,
                message: error.to_string(),
            },
        }
    }

    /// Validate the remembered plan in a scratch copy. `attempt` comes from the
    /// `Intent::Validate` the app produced.
    pub fn validate(
        &self,
        revision: u64,
        attempt: u64,
        sink: &mut dyn FnMut(Stream, &str),
    ) -> Event {
        let Ok(guard) = self.prepared.lock() else {
            return Event::Progress("the adapter's plan slot is poisoned".to_string());
        };
        let Some(prepared) = guard.as_ref() else {
            return Event::Progress("there is no plan to validate".to_string());
        };
        // Discovery resolves `home-manager` from the store on a host that has none
        // on PATH (the audit host does exactly this). Validation must run the same
        // binary, otherwise a store-only installation can never be validated.
        let mut env = self.env.clone();
        if let Some(binary) = nhs_core::toolchain::resolve_toolchain(&RealFs, &self.env)
            .home_manager
            .binary()
        {
            if let Some(dir) = binary.parent() {
                env.path_entries.insert(0, dir.to_path_buf());
            }
        }
        let runner = RealRunner::new(&env);
        match validation::validate(
            &RealFs,
            &prepared.layout,
            &prepared.plan,
            &self.state_dir,
            &runner,
            &self.cancel,
            sink,
        ) {
            Ok(report) => Event::ValidationFinished {
                revision,
                attempt,
                report,
            },
            Err(error) => Event::Progress(format!("validation could not run: {error}")),
        }
    }

    /// Write the remembered plan. The app has already checked the gate; this only
    /// performs it.
    #[must_use]
    pub fn apply(&self, revision: u64) -> Event {
        let Ok(guard) = self.prepared.lock() else {
            return Event::ApplyFinished {
                revision,
                result: ApplyResult::Failed {
                    message: "the adapter's plan slot is poisoned".to_string(),
                },
            };
        };
        let Some(prepared) = guard.as_ref() else {
            return Event::ApplyFinished {
                revision,
                result: ApplyResult::Failed {
                    message: "there is no plan to apply".to_string(),
                },
            };
        };
        let engine = EditEngine::new(
            &RealFs,
            StateLayout {
                state_dir: self.state_dir.clone(),
                config_root: prepared.config_root.clone(),
            },
        );
        let result = match engine.commit(&prepared.plan) {
            Ok(applied) => ApplyResult::Applied {
                snapshot_id: applied.snapshot_id,
                written: applied.written,
            },
            Err(error) => ApplyResult::Failed {
                message: error.to_string(),
            },
        };
        Event::ApplyFinished { revision, result }
    }

    /// Generations and past snapshots, read-only.
    #[must_use]
    pub fn history(&self) -> Event {
        // Generations belong to the user's profile, not to a particular layout:
        // a classic or flake configuration still has them. Listing them here
        // keeps the history screen truthful for every layout.
        let profiles = self.env.state_home().join("nix").join("profiles");
        let state = nhs_core::discovery::generation_state(&RealFs, &profiles);
        let generations = state
            .generations
            .iter()
            .map(|generation| GenerationSummary {
                id: generation.id,
                link: generation.link.clone(),
                target: generation.target.clone(),
                current: generation.id == state.current,
            })
            .collect();
        Event::HistoryLoaded {
            generations,
            snapshots: self.snapshots(),
        }
    }

    /// List past edit snapshots.
    ///
    /// Core has no "list snapshots" call (recorded as an integration gap in
    /// M6.1), so the enumeration lives here: read the directory, load each
    /// record through `SnapshotStore`, keep the ones that load.
    fn snapshots(&self) -> Vec<SnapshotSummary> {
        let store = SnapshotStore::new(&RealFs, &self.state_dir);
        let Ok(entries) = RealFs.read_dir(store.root()) else {
            return Vec::new();
        };
        let mut snapshots: Vec<SnapshotSummary> = entries
            .iter()
            .filter_map(|path| path.file_stem().and_then(|stem| stem.to_str()))
            .filter_map(|id| store.load(id).ok())
            .map(|snapshot| SnapshotSummary {
                id: snapshot.id,
                created_unix_secs: snapshot.created_unix_secs,
                outcome: match snapshot.outcome {
                    nhs_core::edit::snapshot::Outcome::Pending => "pending".to_string(),
                    nhs_core::edit::snapshot::Outcome::Applied => "applied".to_string(),
                    nhs_core::edit::snapshot::Outcome::Failed => "failed".to_string(),
                },
                files: snapshot.changes.len(),
            })
            .collect();
        snapshots.sort_by_key(|snapshot| std::cmp::Reverse(snapshot.created_unix_secs));
        snapshots
    }
}

fn hit(hit: &catalog::Hit<'_>) -> PackageHit {
    PackageHit {
        attr: hit.entry.attr.clone(),
        name: hit.entry.name.clone(),
        version: None,
        description: hit.entry.description.clone(),
        category: Some(hit.entry.category.clone()),
        license: hit.entry.license.clone(),
        platforms: hit.entry.platforms.clone(),
    }
}
