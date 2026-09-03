//! Typed, bounded workspace tools and policy enforcement.
pub mod service;

use poorai_domain::hash_bytes;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::{
    io::Read as _,
    path::{Component, Path, PathBuf},
    time::Duration,
};
use tokio::{process::Command, time::timeout};

/// macOS process-isolation wrapper.
const SEATBELT: &str = "/usr/bin/sandbox-exec";
/// Scratch directory given to child processes, inside the workspace root.
pub const SCRATCH_DIRECTORY: &str = ".poorai-scratch";

#[derive(Debug, thiserror::Error)]
pub enum ToolError {
    #[error("policy denied: {0}")]
    Denied(String),
    #[error("tool I/O failure: {0}")]
    Io(#[from] std::io::Error),
    #[error("tool timed out")]
    Timeout,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "capability", rename_all = "snake_case")]
pub enum ActionProposal {
    Complete {
        rationale: String,
    },
    /// Several replacements in one file, under one hash guard.
    ApplyPatchHunks {
        path: String,
        expected_hash: String,
        hunks: Vec<Hunk>,
    },
    /// Starts a long-running service and waits until it accepts a connection.
    StartService {
        executable: String,
        #[serde(default)]
        args: Vec<String>,
        /// Where to wait for it. Omit and one is reserved and reported back.
        #[serde(default)]
        port: Option<u16>,
        #[serde(default)]
        ready_timeout_secs: Option<u64>,
    },
    /// Stops a service this run started and returns what it printed.
    StopService {
        id: u32,
    },
    /// Creates a directory and its parents.
    MakeDirectory {
        path: String,
    },
    /// Removes a file, guarded by its hash, or a directory it is told to
    /// remove whole.
    DeletePath {
        path: String,
        #[serde(default)]
        expected_hash: Option<String>,
        #[serde(default)]
        recursive: bool,
    },
    /// Moves or renames within the workspace.
    MovePath {
        from: String,
        to: String,
    },
    /// What version control says has changed, structurally.
    VcsStatus {},
    /// The working tree's diff against HEAD, optionally narrowed.
    VcsDiff {
        #[serde(default)]
        paths: Vec<String>,
    },
    /// Offers a command as the deterministic check for this workspace.
    ///
    /// Runs nothing by itself. If a person approves it, it joins the checks the
    /// run verifies against and its executable joins the allowlist; if not, the
    /// workspace still has no verifier and completion is still refused.
    ProposeVerifier {
        executable: String,
        #[serde(default)]
        args: Vec<String>,
        rationale: String,
    },
    ReadFile {
        path: String,
        #[serde(default)]
        first_line: Option<usize>,
        #[serde(default)]
        max_lines: Option<usize>,
    },
    Search {
        query: String,
        max_matches: usize,
    },
    ListTree {
        max_entries: usize,
    },
    ApplyReplace {
        path: String,
        expected_hash: String,
        replacement: String,
    },
    WriteFile {
        path: String,
        content: String,
    },
    ReplaceText {
        path: String,
        expected_hash: String,
        find: String,
        replace: String,
    },
    RunCommand {
        executable: String,
        args: Vec<String>,
        /// Text to write to the command's standard input.
        ///
        /// A command is executed directly rather than through a shell, so
        /// there is no pipe and no redirection: `args` are arguments, never
        /// syntax. That is deliberate -- it is what stops an argument being
        /// reinterpreted as a command -- but it left no way at all to feed a
        /// program its input. Measured: a run that had downloaded a Go
        /// toolchain, built a correct program and then could not test it,
        /// because every attempt to pipe into it was flattened into arguments.
        #[serde(default)]
        stdin: Option<String>,
    },
    FetchUrl {
        url: String,
    },
    /// Marks a plan step finished.
    ///
    /// The deployment says which step it has finished; the harness never infers
    /// it. Inferring would mean the harness deciding the task had progressed,
    /// which is the harness doing the work and would make the measurement
    /// meaningless. This performs nothing in the workspace -- it records a
    /// claim, and the claim is judged against the checks like any other.
    RecordProgress {
        step: usize,
        #[serde(default)]
        note: Option<String>,
    },
}
impl ActionProposal {
    pub fn validate(&self) -> Result<(), ToolError> {
        match self {
            Self::Complete { rationale } if rationale.is_empty() => {
                Err(ToolError::Denied("completion rationale is required".into()))
            }
            Self::ProposeVerifier {
                executable,
                rationale,
                ..
            } => {
                if executable.trim().is_empty() || rationale.trim().is_empty() {
                    return Err(ToolError::Denied(
                        "a proposed verifier needs an executable and a rationale a person can judge"
                            .into(),
                    ));
                }
                // The same shape refused for run_command: a program name never
                // contains whitespace, and one that does reaches exec as a
                // single filename.
                if executable.split_whitespace().count() > 1 {
                    return Err(ToolError::Denied(format!(
                        "`{executable}` is a command line where a program name belongs; pass the program in `executable` and the rest in `args`"
                    )));
                }
                Ok(())
            }
            Self::ReadFile { path, .. }
            | Self::ApplyReplace { path, .. }
            | Self::WriteFile { path, .. }
            | Self::ReplaceText { path, .. }
                if path.is_empty() =>
            {
                Err(ToolError::Denied("action path is empty".into()))
            }
            Self::Search { query, max_matches } if query.is_empty() || *max_matches == 0 => Err(
                ToolError::Denied("search query and limit are required".into()),
            ),
            Self::ListTree { max_entries } if *max_entries == 0 => {
                Err(ToolError::Denied("tree limit is required".into()))
            }
            Self::ReadFile {
                max_lines: Some(0), ..
            } => Err(ToolError::Denied(
                "max_lines must be greater than zero".into(),
            )),
            Self::ReplaceText { find, .. } if find.is_empty() => {
                Err(ToolError::Denied("find text is required".into()))
            }
            Self::FetchUrl { url } if url.is_empty() => {
                Err(ToolError::Denied("url is required".into()))
            }
            Self::RunCommand { executable, .. } if executable.is_empty() => {
                Err(ToolError::Denied("executable is required".into()))
            }
            _ => Ok(()),
        }
    }
}
/// Process-level isolation for command execution.
///
/// The boundary is recorded on every result rather than assumed: a run that
/// could not be sandboxed must be visibly unsandboxed, never silently so.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SandboxPolicy {
    /// Refuse to run a command at all when no sandbox is available.
    Required,
    /// Sandbox where the platform supports it, and record when it did not.
    Preferred,
    Disabled,
}

/// Actions whose effects reach past the workspace, and which a user must
/// authorise explicitly. Denial is the default; a grant is never inferred.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum Approval {
    /// Editing a dependency manifest or lockfile.
    DependencyChange,
    /// Adopting a command the agent proposed as this workspace's verifier.
    ///
    /// A repository that declares no checks cannot complete, which is correct
    /// and leaves no way forward: the two toolchain-provisioning runs wrote
    /// correct programs into workspaces created from nothing and are failures
    /// under that rule. The way out cannot be the agent running whatever it
    /// nominates -- a command nobody authorised is not a verifier, and one the
    /// agent both chooses and trusts is the agent marking its own work. So it
    /// proposes and a person decides.
    VerifierProposal,
    /// Rewriting version-control history.
    HistoryRewrite,
    /// Publishing a package or pushing to a remote.
    Publish,
    /// Reaching the network at all, for the workspace and for any process it
    /// runs. Dependency resolution needs it; so does exfiltration.
    NetworkAccess,
    /// Binding and connecting to services on this machine, and nothing beyond
    /// it.
    ///
    /// Verifying a system rather than a file means starting a service and
    /// exercising it, which needs to reach a local port. It is separate from
    /// `NetworkAccess` because it is a genuinely smaller grant -- no remote
    /// host is reachable, so nothing can be exfiltrated -- and a genuinely
    /// real one: it reaches whatever else listens on this machine, the model
    /// backend among it. Neither implies the other, and the audit records
    /// which was given.
    ///
    /// The boundary is this *host*, not the loopback interface. seatbelt takes
    /// only `*` or `localhost` as the host in a network address, and its
    /// `localhost` covers every address the machine holds, so a service on a
    /// LAN interface is in scope too. That is the platform's limit rather than
    /// an intent, and it is stated here because the narrower reading would be
    /// wrong.
    LocalService,
    /// Running any executable, so a run can fetch and install the toolchain a
    /// task needs -- a JDK, a Go distribution, a Flutter SDK -- rather than
    /// failing because the host happens not to have it.
    ///
    /// This is the widest grant poorAI has, and what makes it defensible is
    /// where the installs land rather than what they are. A child process
    /// already runs with `HOME` and `TMPDIR` inside the workspace, and the
    /// sandbox already confines writes there, so a toolchain installed under
    /// this grant is installed *into the workspace*: the host is not modified,
    /// nothing persists into the next run, and deleting the workspace undoes
    /// it. That is a better arrangement than installing to the host even
    /// setting safety aside.
    ///
    /// What it does not do is make a run safe to combine with
    /// `NetworkAccess` and leave unattended. The sandbox denies writing
    /// outside the workspace; it does not deny *reading* outside it. An
    /// arbitrary executable plus the network is the shape of an exfiltration,
    /// and that is a property of the pair rather than of either alone. The
    /// sensitive parts of the host home directory are denied to any sandboxed
    /// run (see `sandbox_profile`), which narrows it but does not close it.
    ToolchainInstall,
}

/// Host paths under the real home directory that no run has a reason to read.
///
/// Denied to every sandboxed run. `HOME` is redirected into the workspace, so
/// a tool looking for its own configuration finds the workspace copy; these are
/// the absolute paths that would go around that.
const NEVER_READABLE: [&str; 9] = [
    ".ssh",
    ".aws",
    ".gnupg",
    ".config/gh",
    ".config/gcloud",
    ".kube",
    ".docker/config.json",
    ".netrc",
    "Library/Keychains",
];

/// System paths a process needs to exist at all.
///
/// A sandbox that denies reading everything outside the workspace also denies
/// the dynamic linker its cache and the shell its binaries, and nothing starts.
/// This is the floor: enough to load and run a program, and no user data.
const SYSTEM_READABLE: [&str; 10] = [
    // The root directory itself: without it nothing resolves a path and no
    // process starts at all. Measured -- `/usr/bin/true` aborts without it.
    "/",
    "/usr",
    "/bin",
    "/sbin",
    "/System",
    "/Library",
    "/private/var/db",
    // `xcode-select` reads its developer directory link from here, and git
    // refuses to run without it.
    "/private/var/select",
    "/private/etc",
    "/dev",
];

/// Where toolchains live outside the workspace.
///
/// The command allowlist is derived from the repository, and what it names is
/// usually installed in the user's home -- cargo, rustup, pyenv, nvm. Denying
/// the home wholesale would deny the agent the compiler it was told to use, so
/// these subpaths are readable and the rest of the home is not. A toolchain
/// poorAI installed itself lives inside the workspace and needs no entry.
const TOOLCHAIN_READABLE: [&str; 10] = [
    ".cargo", ".rustup", ".local", ".pyenv", ".nvm", ".sdkman", ".gradle", ".m2", "go", ".bun",
];

/// Toolchain roots outside the home, by convention on this platform.
const TOOLCHAIN_PREFIXES: [&str; 3] = ["/opt/homebrew", "/opt/local", "/Applications/Xcode.app"];

/// Dependency manifests and lockfiles. Editing one changes what the build
/// fetches and executes, so it is an approval gate rather than a plain edit.
const DEPENDENCY_MANIFESTS: [&str; 12] = [
    "Cargo.toml",
    "Cargo.lock",
    "package.json",
    "package-lock.json",
    "yarn.lock",
    "pnpm-lock.yaml",
    "requirements.txt",
    "pyproject.toml",
    "poetry.lock",
    "go.mod",
    "go.sum",
    "Gemfile",
];

/// Git subcommands and flags that discard or rewrite recorded history.
const HISTORY_REWRITE_ARGS: [&str; 6] = [
    "rebase",
    "filter-branch",
    "filter-repo",
    "--force",
    "-f",
    "--amend",
];

/// Returns the approval a proposed command requires, if any.
pub fn command_approval(executable: &str, args: &[String]) -> Option<Approval> {
    let name = Path::new(executable)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| executable.to_string());
    if args.iter().any(|arg| arg == "publish") {
        return Some(Approval::Publish);
    }
    if name == "git" {
        if args.iter().any(|arg| arg == "push") {
            return Some(Approval::Publish);
        }
        if args
            .iter()
            .any(|arg| HISTORY_REWRITE_ARGS.contains(&arg.as_str()))
        {
            return Some(Approval::HistoryRewrite);
        }
        // Both of these discard uncommitted work, which is the agent's own
        // output as often as the user's. The comment here used to name `clean`
        // while only `reset` was checked, so the destructive half nobody had
        // written down was the one that ran.
        if args.iter().any(|arg| arg == "reset") && args.iter().any(|arg| arg == "--hard") {
            return Some(Approval::HistoryRewrite);
        }
        if args.iter().any(|arg| arg == "clean") {
            return Some(Approval::HistoryRewrite);
        }
    }
    None
}

/// What an action requires, and a description a person can judge.
///
/// One place, so the loop can ask before acting rather than each tool
/// discovering its own gate at the moment it would have acted.
pub fn required_approval(action: &ActionProposal) -> Option<(Approval, String)> {
    match action {
        ActionProposal::FetchUrl { url } => Some((Approval::NetworkAccess, format!("fetch {url}"))),
        ActionProposal::RunCommand {
            executable, args, ..
        } => command_approval(executable, args)
            .map(|approval| (approval, format!("run `{executable} {}`", args.join(" ")))),
        ActionProposal::ApplyReplace { path, .. } | ActionProposal::WriteFile { path, .. } => {
            edit_approval(Path::new(path)).map(|a| (a, format!("write {path}")))
        }
        ActionProposal::ReplaceText { path, find, .. } => edit_approval(Path::new(path))
            .map(|a| (a, format!("change {path} where it reads `{}`", elide(find)))),
        ActionProposal::ProposeVerifier {
            executable,
            args,
            rationale,
        } => Some((
            Approval::VerifierProposal,
            format!(
                "adopt `{executable} {}` as this workspace's verifier — {}",
                args.join(" "),
                elide(rationale)
            ),
        )),
        _ => None,
    }
}

/// Shortens a fragment for a prompt without hiding what it is.
fn elide(text: &str) -> String {
    let single_line = text.replace('\n', " ");
    if single_line.chars().count() <= 60 {
        return single_line;
    }
    format!("{}…", single_line.chars().take(59).collect::<String>())
}

/// Returns the approval editing `relative` requires, if any.
pub fn edit_approval(relative: &Path) -> Option<Approval> {
    let name = relative.file_name()?.to_string_lossy().to_string();
    DEPENDENCY_MANIFESTS
        .contains(&name.as_str())
        .then_some(Approval::DependencyChange)
}

#[derive(Debug, Clone)]
pub struct ToolPolicy {
    pub root: PathBuf,
    /// Paths outside the root a command may read, named by whoever built the
    /// policy.
    ///
    /// Empty for a task: an agent working in a repository has no business
    /// reading elsewhere. Corpus preparation is the case that needs it, and
    /// only for the source the corpus itself declares -- a local mirror it was
    /// told to clone from. Naming it here rather than widening the profile
    /// keeps the exception to exactly what was declared.
    pub extra_readable: Vec<PathBuf>,
    pub allow_commands: Vec<String>,
    pub output_limit: usize,
    pub timeout: Duration,
    pub sandbox: SandboxPolicy,
    /// Approvals the user has explicitly granted for this run.
    pub approvals: Vec<Approval>,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PolicyProfile {
    Safe,
    Development,
}
impl PolicyProfile {
    pub fn build(self, root: PathBuf) -> ToolPolicy {
        let allow_commands = match self {
            Self::Safe => vec![],
            Self::Development => vec!["cargo".into(), "git".into(), "rg".into()],
        };
        ToolPolicy {
            root,
            extra_readable: Vec::new(),
            allow_commands,
            output_limit: 64 * 1024,
            timeout: Duration::from_secs(120),
            sandbox: SandboxPolicy::Preferred,
            // Nothing beyond the workspace is authorised until a user says so.
            approvals: Vec::new(),
        }
    }
}
impl ToolPolicy {
    pub fn resolve(&self, relative: &Path) -> Result<PathBuf, ToolError> {
        if relative.is_absolute()
            || relative
                .components()
                .any(|c| matches!(c, Component::ParentDir))
        {
            return Err(ToolError::Denied("path escapes workspace root".into()));
        }
        let root = self
            .root
            .canonicalize()
            .map_err(|_| ToolError::Denied("workspace root is unavailable".into()))?;
        let result = root.join(relative);
        let checked = if result.exists() {
            result
                .canonicalize()
                .map_err(|_| ToolError::Denied("unable to resolve target".into()))?
        } else {
            // The target does not exist yet, and neither may several of its
            // parents: creating `src/app.js` in an empty workspace has no `src`
            // to canonicalise. Resolve the deepest ancestor that does exist,
            // which is where any symlink could redirect the path, and rebuild
            // the rest onto it. `..` and absolute paths were already refused,
            // so the remainder cannot climb back out.
            let mut existing = result.as_path();
            let mut trailing = Vec::new();
            while !existing.exists() {
                trailing.push(
                    existing
                        .file_name()
                        .ok_or_else(|| ToolError::Denied("target has no file name".into()))?
                        .to_owned(),
                );
                existing = existing
                    .parent()
                    .ok_or_else(|| ToolError::Denied("target has no parent".into()))?;
            }
            let mut resolved = existing
                .canonicalize()
                .map_err(|_| ToolError::Denied("target parent is unavailable".into()))?;
            for component in trailing.iter().rev() {
                resolved.push(component);
            }
            resolved
        };
        if !checked.starts_with(&root) {
            return Err(ToolError::Denied("path escapes workspace root".into()));
        }
        Ok(checked)
    }
    /// Whether this run may reach the network.
    ///
    /// Derived from the grant rather than stored separately: a policy with the
    /// network open and no approval recorded would be a boundary nobody
    /// authorised, and the audit would not show who did.
    pub fn network_allowed(&self) -> bool {
        self.approvals.contains(&Approval::NetworkAccess)
    }
    /// Denies unless the user granted this approval for the run.
    pub fn require(&self, approval: Approval) -> Result<(), ToolError> {
        if self.approvals.contains(&approval) {
            return Ok(());
        }
        Err(ToolError::Denied(format!(
            "{approval:?} requires explicit user approval"
        )))
    }
    /// Builds a seatbelt profile confining writes to the workspace root.
    ///
    /// Returns `None` when the platform offers no sandbox, or when the root
    /// cannot be expressed safely in a profile.
    fn sandbox_profile(&self) -> Option<String> {
        if !cfg!(target_os = "macos") || !Path::new(SEATBELT).is_file() {
            return None;
        }
        // The canonical path is required: on macOS `/tmp` is a symlink, and a
        // profile written against the uncanonical path grants nothing.
        let root = self.root.canonicalize().ok()?;
        let root = root.to_str()?;
        // A root that cannot be quoted safely would let the path itself rewrite
        // the profile, so refuse rather than emit a weakened one.
        if root.contains('"') || root.contains('\\') {
            return None;
        }
        // Order matters in a seatbelt profile: the last matching rule wins, so
        // the loopback allowance must follow the blanket denial it carves out
        // of. Written the other way round it grants nothing.
        // Nothing a run legitimately does needs the host's credentials, and a
        // sandbox that confines writes while leaving reads open is one half of
        // an exfiltration. `HOME` already points inside the workspace, so a
        // well-behaved tool reads the workspace copy of these and never the
        // host's; this denies the absolute paths that bypass that. Applied to
        // every sandboxed run rather than only to a grant, because there is no
        // run for which reading them would have been right.
        let quotable = |path: &std::path::Path| -> Option<String> {
            let path = path.to_str()?;
            (!path.contains('"') && !path.contains('\\')).then(|| format!("(subpath \"{path}\")"))
        };
        // The host's real home, which is not where the child's HOME points.
        let host_home = std::env::var_os("HOME").map(std::path::PathBuf::from);
        let mut secrets = String::new();
        if let Some(home) = &host_home {
            let subpaths: Vec<String> = NEVER_READABLE
                .iter()
                .map(|relative| home.join(relative))
                .filter_map(|path| quotable(&path))
                .collect();
            if !subpaths.is_empty() {
                secrets = format!("(deny file-read* {})", subpaths.join(""));
            }
            // Kept even though the home is now unreadable by default: these
            // are the paths for which no run has a reason, and a later
            // allowlist entry that widened the home would otherwise reopen
            // them silently.
        }
        // Reads were open. The sandbox confined writes and denied the network
        // and nine known credential paths, and left everything else on the
        // machine legible -- which, with `--provision` granting an arbitrary
        // executable and a network together, is the shape of an exfiltration.
        //
        // So reading is denied by default and opened deliberately: the system
        // paths a process needs to start, the workspace, and the toolchain
        // directories the derived command allowlist actually names. The rest of
        // the home -- documents, mail, browser profiles, other repositories --
        // is no longer readable by a command the agent runs.
        let mut readable: Vec<String> = SYSTEM_READABLE
            .iter()
            .chain(TOOLCHAIN_PREFIXES.iter())
            .map(std::path::Path::new)
            .filter_map(|path| {
                // The root is a literal, not a subpath: as a subpath it would
                // re-open everything the denial just closed.
                if path == std::path::Path::new("/") {
                    return Some("(literal \"/\")".to_string());
                }
                quotable(path)
            })
            .collect();
        readable.push(format!("(subpath \"{root}\")"));
        readable.extend(
            self.extra_readable
                .iter()
                .filter_map(|path| path.canonicalize().ok())
                .filter_map(|path| quotable(&path)),
        );
        if let Some(home) = &host_home {
            readable.extend(
                TOOLCHAIN_READABLE
                    .iter()
                    .map(|relative| home.join(relative))
                    .filter_map(|path| quotable(&path)),
            );
        }
        // Data rather than every read. Resolving a path walks its components,
        // and denying metadata denies that walk -- the executable itself stops
        // being findable, which is a broken sandbox rather than a strict one.
        // Denying the data still closes what matters: contents are unreadable
        // and a directory cannot be listed, so a command can neither read the
        // user's files nor enumerate them.
        let reads = format!(
            "(deny file-read-data)(allow file-read-data {})",
            readable.join("")
        );
        let network = if self.network_allowed() {
            String::new()
        } else if self.approvals.contains(&Approval::LocalService) {
            // `localhost` is not loopback, and cannot be narrowed to it.
            // seatbelt accepts only `*` or `localhost` as the host in a
            // network address -- a literal `127.0.0.1` is rejected and the
            // whole profile fails to compile -- and its `localhost` means this
            // *host*: every address the machine holds, its LAN interfaces
            // included. So this grant reaches a service listening on this
            // machine's LAN address, not only on loopback, and a process under
            // it can be reached from the LAN if it binds there.
            //
            // That is wider than the name suggests and is the platform's
            // limit, not a choice. What it still withholds is the part that
            // matters: a genuinely remote host is denied, so nothing leaves
            // the machine. A fixture asserts that denial by its error --
            // `PermissionError` from the sandbox rather than a timeout --
            // because an earlier version aimed at a public address passed
            // vacuously when the connection merely timed out.
            //
            // All three operations, because a service needs every one: bind to
            // take the port, inbound to listen and accept, outbound to be
            // connected to. Granting bind alone lets a server claim a port and
            // then fail at `listen`, which another fixture caught.
            "(deny network*)(allow network-bind (local ip \"localhost:*\"))\
             (allow network-inbound (local ip \"localhost:*\"))\
             (allow network-outbound (remote ip \"localhost:*\"))"
                .to_string()
        } else {
            "(deny network*)".to_string()
        };
        // Order is load-bearing: the last matching rule wins, so the reads
        // allowlist follows its blanket denial and the credential denial
        // follows the allowlist -- otherwise a key under a readable toolchain
        // directory would be readable again.
        Some(format!(
            "(version 1)(allow default)(deny file-write*)(allow file-write* (subpath \"{root}\"))(allow file-write-data (literal \"/dev/null\") (literal \"/dev/stdout\") (literal \"/dev/stderr\")){reads}{secrets}{network}"
        ))
    }
    /// Whether a command will actually be sandboxed, refusing if it must be
    /// and cannot.
    pub fn will_sandbox(&self) -> Result<bool, ToolError> {
        let available = match self.sandbox {
            SandboxPolicy::Disabled => None,
            SandboxPolicy::Preferred | SandboxPolicy::Required => self.sandbox_profile(),
        };
        if available.is_none() && self.sandbox == SandboxPolicy::Required {
            return Err(ToolError::Denied(
                "policy requires a sandbox and this platform provides none".into(),
            ));
        }
        Ok(available.is_some())
    }

    /// Builds a child command with the boundary already applied.
    ///
    /// One place, because a second way of starting a process is a second
    /// chance to forget the sandbox, the scratch directory or the cleared
    /// environment. A long-running service goes through exactly this.
    pub fn prepare_command(&self, executable: &str, args: &[String]) -> Result<Command, ToolError> {
        let profile = match self.sandbox {
            SandboxPolicy::Disabled => None,
            SandboxPolicy::Preferred | SandboxPolicy::Required => self.sandbox_profile(),
        };
        if profile.is_none() && self.sandbox == SandboxPolicy::Required {
            return Err(ToolError::Denied(
                "policy requires a sandbox and this platform provides none".into(),
            ));
        }
        let mut command = match &profile {
            Some(profile) => {
                let mut wrapped = Command::new(SEATBELT);
                wrapped.arg("-p").arg(profile).arg(executable).args(args);
                wrapped
            }
            None => {
                let mut plain = Command::new(executable);
                plain.args(args);
                plain
            }
        };
        // Build tooling needs a scratch directory -- rustdoc creates one per
        // doctest run -- and the system one lies outside the sandbox. Rather
        // than widening the boundary to all of $TMPDIR, which would let one
        // workspace write into another's, the child is given a scratch
        // directory inside its own root.
        //
        // The canonical root, for the same reason the seatbelt profile needs
        // it: on macOS the uncanonical path is a symlink, and handing the
        // child that path makes its scratch directory look like it lies
        // outside the workspace.
        let scratch = self
            .root
            .canonicalize()
            .unwrap_or_else(|_| self.root.clone())
            .join(SCRATCH_DIRECTORY);
        let _ = std::fs::create_dir_all(&scratch);
        command
            .current_dir(&self.root)
            .env_clear()
            // PATH is allowlisted solely for executable resolution; never logged.
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("TMPDIR", &scratch)
            .env("TMP", &scratch)
            .env("TEMP", &scratch)
            // Package managers keep caches and config under HOME. Pointing it
            // into the workspace keeps them inside the boundary instead of
            // widening it, and makes a run hermetic: nothing it downloads
            // persists into the next one, and nothing in the real home
            // directory is read.
            .env("HOME", &scratch);
        Ok(command)
    }

    pub fn redact(&self, text: &str) -> (String, bool) {
        let re = Regex::new(r"(?i)(api[_-]?key|token|password)\s*[=:]\s*[^\s]+|AKIA[0-9A-Z]{16}")
            .expect("valid regex");
        let result = re.replace_all(text, "[REDACTED]").to_string();
        let changed = result != text;
        (result, changed)
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub duration_ms: u128,
    pub redacted: bool,
    pub artifact_hash: String,
    #[serde(default)]
    pub stdout_truncated: bool,
    #[serde(default)]
    pub stderr_truncated: bool,
    /// Whether the process actually ran inside a sandbox. Recorded rather than
    /// assumed so an unsandboxed run is visible in the audit.
    pub sandboxed: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileReadResult {
    pub path: String,
    pub content: String,
    pub truncated: bool,
    /// Hash of the whole file, not of the window returned. An edit is guarded
    /// against the file as it is on disk, so a partial read still yields a
    /// usable hash. Repeated under `expected_hash` because that is the name of
    /// the parameter it must be passed to.
    pub artifact_hash: String,
    pub expected_hash: String,
    pub redacted: bool,
    /// Lines in the whole file, so a caller can tell it has seen only part.
    pub total_lines: usize,
    /// One-based line the returned window starts at.
    pub first_line: usize,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchMatch {
    pub path: String,
    pub line: usize,
    pub excerpt: String,
    pub redacted: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TreeEntry {
    pub path: String,
    pub directory: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApplyResult {
    pub path: String,
    pub previous_hash: String,
    pub new_hash: String,
    /// The same value as `new_hash`, under the name of the parameter that
    /// consumes it. A result field called `new_hash` and a parameter called
    /// `expected_hash` are one value under two names, and the mapping has to
    /// be inferred. Measured: a model re-sent the pre-edit hash four times
    /// after a successful edit, having never made that inference.
    pub expected_hash: String,
}

/// Lists a bounded, policy-filtered workspace tree.
/// Directories excluded whatever the repository says about them.
/// Files above this are inventory, not searchable text.
const MAX_SEARCHED_BYTES: u64 = 1_000_000;

const POLICY_EXCLUSIONS: [&str; 4] = [".git", "target", "node_modules", ".poorai"];

/// Walks the workspace the way the index does.
///
/// `Search` and `ListTree` used to do their own `read_dir` and skip four known
/// directory names, while the repository index walked under full gitignore
/// semantics. A file deliberately excluded from retrieval -- an environment
/// file among them -- was therefore still reachable through a tool, which is
/// the ignore rules holding in one direction only.
///
/// Order is by path rather than by whatever the filesystem returns, so two
/// listings of an unchanged workspace are the same listing. A tool result that
/// feeds a prompt should not depend on directory order.
fn walk_workspace(root: &Path) -> Result<Vec<(PathBuf, bool)>, ToolError> {
    let walker = ignore::WalkBuilder::new(root)
        .hidden(false)
        .git_ignore(true)
        // A workspace is untrusted input whether or not it is a checkout.
        .require_git(false)
        .git_global(false)
        .git_exclude(true)
        .parents(false)
        .follow_links(false)
        .sort_by_file_path(|a, b| a.cmp(b))
        .filter_entry(|entry| {
            !POLICY_EXCLUSIONS
                .iter()
                .any(|blocked| entry.file_name() == *blocked)
        })
        .build();
    let mut entries = Vec::new();
    for entry in walker {
        let entry =
            entry.map_err(|error| ToolError::Denied(format!("unreadable path: {error}")))?;
        let path = entry.path().to_path_buf();
        if path == root {
            continue;
        }
        // symlink_metadata keeps a link pointing outside the root from being
        // presented as though it lived inside it.
        let metadata = std::fs::symlink_metadata(&path)?;
        if metadata.file_type().is_symlink() {
            continue;
        }
        entries.push((path, metadata.is_dir()));
    }
    Ok(entries)
}

pub fn list_tree(policy: &ToolPolicy, max_entries: usize) -> Result<Vec<TreeEntry>, ToolError> {
    let deadline = std::time::Instant::now() + policy.timeout;
    let mut output = Vec::new();
    let mut output_bytes = 0usize;
    for (path, directory) in walk_workspace(&policy.root)? {
        if std::time::Instant::now() >= deadline {
            return Err(ToolError::Timeout);
        }
        if output.len() >= max_entries {
            break;
        }
        let relative = path
            .strip_prefix(&policy.root)
            .unwrap_or(&path)
            .display()
            .to_string();
        let cost = relative.len().saturating_add(1);
        if output_bytes.saturating_add(cost) > policy.output_limit {
            break;
        }
        output_bytes = output_bytes.saturating_add(cost);
        output.push(TreeEntry {
            path: relative,
            directory,
        });
    }
    Ok(output)
}

/// One replacement within a file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hunk {
    pub find: String,
    pub replace: String,
}

/// Applies several replacements to one file under a single hash guard.
///
/// A change touching three places in a file was three whole-file rewrites,
/// each carrying the entire file and each invalidating the hash the next one
/// was written against -- so the second and third arrived stale and the run
/// spent its budget re-reading. This is the same guard, once.
///
/// Every hunk must match exactly once, and all of them are checked before any
/// is applied: a patch that half-lands leaves a file in a state nobody
/// described, which is worse than one that does not land at all.
pub fn apply_patch(
    policy: &ToolPolicy,
    relative: &Path,
    expected_hash: &str,
    hunks: &[Hunk],
) -> Result<ApplyResult, ToolError> {
    if hunks.is_empty() {
        return Err(ToolError::Denied("a patch needs at least one hunk".into()));
    }
    if let Some(approval) = edit_approval(relative) {
        policy.require(approval)?;
    }
    let path = policy.resolve(relative)?;
    let original = std::fs::read(&path)?;
    if original.iter().take(4096).any(|byte| *byte == 0) {
        return Err(ToolError::Denied("refusing to patch a binary file".into()));
    }
    let current = hash_bytes(&original);
    if current != expected_hash {
        return Err(ToolError::Denied(format!(
            "stale file hash; the file now hashes to {current}. Reread it before patching."
        )));
    }
    let mut content = String::from_utf8_lossy(&original).to_string();

    // Checked first, applied second. A hunk that would match text an earlier
    // hunk introduced is not the caller's intent, and finding that out halfway
    // through is finding it out too late.
    for (index, hunk) in hunks.iter().enumerate() {
        if hunk.find.is_empty() {
            return Err(ToolError::Denied(format!(
                "hunk {} has nothing to find",
                index + 1
            )));
        }
        match content.matches(&hunk.find).count() {
            1 => {}
            0 if content.contains(&hunk.replace) => {
                return Err(ToolError::Denied(format!(
                    "hunk {} is already applied: its replacement is present and its find text is not",
                    index + 1
                )));
            }
            0 => {
                return Err(ToolError::Denied(format!(
                    "hunk {} does not appear in {}",
                    index + 1,
                    relative.display()
                )));
            }
            found => {
                return Err(ToolError::Denied(format!(
                    "hunk {} matches {found} times; make it unique so the edit is not the wrong one",
                    index + 1
                )));
            }
        }
    }
    for hunk in hunks {
        content = content.replacen(&hunk.find, &hunk.replace, 1);
    }
    if content.len() > policy.output_limit {
        return Err(ToolError::Denied(
            "patched content exceeds policy size limit".into(),
        ));
    }
    std::fs::write(&path, content.as_bytes())?;
    let new_hash = hash_bytes(content.as_bytes());
    Ok(ApplyResult {
        path: relative.display().to_string(),
        previous_hash: expected_hash.to_string(),
        expected_hash: new_hash.clone(),
        new_hash,
    })
}

/// What a filesystem change did, for the audit and the next call.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PathChange {
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    /// Files affected, so a caller can tell one file from a directory tree.
    pub entries: usize,
}

/// Creates a directory, and its parents, inside the workspace.
///
/// The surface was read, create and replace, so a task that reorganises files
/// could not be expressed at all -- the agent could write a file into a new
/// directory but never make an empty one, move anything, or remove what it had
/// superseded.
pub fn make_directory(policy: &ToolPolicy, relative: &Path) -> Result<PathChange, ToolError> {
    let path = policy.resolve(relative)?;
    if path.is_file() {
        return Err(ToolError::Denied(
            "a file already exists at that path".into(),
        ));
    }
    std::fs::create_dir_all(&path)?;
    Ok(PathChange {
        path: relative.display().to_string(),
        from: None,
        entries: 0,
    })
}

/// Deletes a file, or a directory and what it contains.
///
/// Guarded by the hash of what is being removed, exactly as an edit is: a
/// delete is the most irreversible edit there is, and "the file I read" and
/// "the file on disk" being different matters more here than anywhere else.
/// A directory has no single hash, so removing one is deliberate rather than
/// guarded -- `recursive` has to be asked for, and the count of what went is
/// returned so the audit says how much.
pub fn delete_path(
    policy: &ToolPolicy,
    relative: &Path,
    expected_hash: Option<&str>,
    recursive: bool,
) -> Result<PathChange, ToolError> {
    let path = policy.resolve(relative)?;
    let metadata = std::fs::symlink_metadata(&path)?;
    if metadata.file_type().is_symlink() {
        return Err(ToolError::Denied(
            "refusing to delete a symlink; it may point outside the workspace".into(),
        ));
    }
    if metadata.is_dir() {
        if !recursive {
            return Err(ToolError::Denied(format!(
                "{} is a directory; pass recursive to remove it and everything in it",
                relative.display()
            )));
        }
        let entries = walk_workspace(&path)?.len();
        std::fs::remove_dir_all(&path)?;
        return Ok(PathChange {
            path: relative.display().to_string(),
            from: None,
            entries,
        });
    }
    let current = hash_bytes(std::fs::read(&path)?);
    match expected_hash {
        Some(expected) if expected == current => {}
        Some(_) => {
            return Err(ToolError::Denied(format!(
                "stale file hash; the file now hashes to {current}. Reread it before deleting."
            )));
        }
        None => {
            return Err(ToolError::Denied(
                "deleting a file needs its current hash, as editing one does".into(),
            ));
        }
    }
    if let Some(approval) = edit_approval(relative) {
        policy.require(approval)?;
    }
    std::fs::remove_file(&path)?;
    Ok(PathChange {
        path: relative.display().to_string(),
        from: None,
        entries: 1,
    })
}

/// Moves or renames a path within the workspace.
///
/// Both ends are resolved against the root, so neither reaches outside it, and
/// an existing destination is refused rather than overwritten -- the same rule
/// `write_file` follows, and for the same reason: a blind overwrite should
/// never be one missing argument away.
pub fn move_path(policy: &ToolPolicy, from: &Path, to: &Path) -> Result<PathChange, ToolError> {
    let source = policy.resolve(from)?;
    let destination = policy.resolve(to)?;
    if !source.exists() {
        return Err(ToolError::Denied(format!(
            "{} does not exist",
            from.display()
        )));
    }
    if destination.exists() {
        return Err(ToolError::Denied(format!(
            "{} already exists; delete it first if replacing it is intended",
            to.display()
        )));
    }
    if let Some(approval) = edit_approval(from).or_else(|| edit_approval(to)) {
        policy.require(approval)?;
    }
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let entries = if source.is_dir() {
        walk_workspace(&source)?.len()
    } else {
        1
    };
    std::fs::rename(&source, &destination)?;
    Ok(PathChange {
        path: to.display().to_string(),
        from: Some(from.display().to_string()),
        entries,
    })
}

/// Replaces text only when the caller supplies the current content hash, preventing stale edits.
pub fn apply_replace(
    policy: &ToolPolicy,
    relative: &Path,
    expected_hash: &str,
    replacement: &str,
) -> Result<ApplyResult, ToolError> {
    if let Some(approval) = edit_approval(relative) {
        policy.require(approval)?;
    }
    let path = policy.resolve(relative)?;
    if std::fs::metadata(&path)?.len()
        > u64::try_from(policy.output_limit.saturating_mul(16)).unwrap_or(u64::MAX)
    {
        return Err(ToolError::Denied(
            "existing file exceeds the bounded edit size limit".into(),
        ));
    }
    let existing = std::fs::read(&path)?;
    if existing.iter().take(4096).any(|b| *b == 0) {
        return Err(ToolError::Denied("binary edits are denied".into()));
    }
    let previous_hash = hash_bytes(&existing);
    if previous_hash != expected_hash {
        return Err(ToolError::Denied(format!(
            "stale file hash; the file now hashes to {previous_hash}"
        )));
    }
    if replacement.len() > policy.output_limit {
        return Err(ToolError::Denied(
            "replacement exceeds policy size limit".into(),
        ));
    }
    std::fs::write(&path, replacement.as_bytes())?;
    let new_hash = hash_bytes(replacement.as_bytes());
    Ok(ApplyResult {
        path: relative.display().to_string(),
        previous_hash,
        expected_hash: new_hash.clone(),
        new_hash,
    })
}

/// Replaces one exact occurrence of `find` with `replace`.
///
/// Whole-file replacement cannot reach a real repository: changing one line of
/// a two-thousand-line file would mean re-emitting the whole file, which is
/// impractical inside any context budget. This edits in place.
///
/// The match must be unique. Two occurrences mean the caller may not have
/// meant the one that would be changed, and guessing which is exactly the kind
/// of silent wrong edit the hash guard exists to prevent.
pub fn replace_text(
    policy: &ToolPolicy,
    relative: &Path,
    expected_hash: &str,
    find: &str,
    replace: &str,
) -> Result<ApplyResult, ToolError> {
    if let Some(approval) = edit_approval(relative) {
        policy.require(approval)?;
    }
    if find.is_empty() {
        return Err(ToolError::Denied("find text is empty".into()));
    }
    let path = policy.resolve(relative)?;
    if std::fs::metadata(&path)?.len()
        > u64::try_from(policy.output_limit.saturating_mul(16)).unwrap_or(u64::MAX)
    {
        return Err(ToolError::Denied(
            "existing file exceeds the bounded edit size limit".into(),
        ));
    }
    let existing = std::fs::read(&path)?;
    if existing.iter().take(4096).any(|b| *b == 0) {
        return Err(ToolError::Denied("binary edits are denied".into()));
    }
    let previous_hash = hash_bytes(&existing);
    if previous_hash != expected_hash {
        // The current hash is in hand, and withholding it makes the caller
        // spend a turn re-reading to learn something the refusal already knew.
        // Measured: three consecutive refusals of this kind in one run, each
        // costing an action the run did not have to spare.
        return Err(ToolError::Denied(format!(
            "stale file hash; the file now hashes to {previous_hash}"
        )));
    }
    let text = String::from_utf8_lossy(&existing).to_string();
    let occurrences = text.matches(find).count();
    match occurrences {
        0 => {
            // "Not found" is true but unhelpful when the reason it is absent
            // is that this very edit already replaced it. Saying so is the
            // difference between a caller that moves on and one that retries
            // the same edit until its budget runs out. Measured: exactly that
            // loop, four times in one run, on a file already correctly fixed.
            if !replace.is_empty() && text.contains(replace) {
                return Err(ToolError::Denied(format!(
                    "this edit is already applied: the file does not contain the `find` text \
                     and does contain the `replace` text. The file now hashes to {previous_hash}"
                )));
            }
            return Err(ToolError::Denied(
                "find text does not appear in the file".into(),
            ));
        }
        1 => {}
        many => {
            return Err(ToolError::Denied(format!(
                "find text appears {many} times; include enough surrounding text to be unique"
            )));
        }
    }
    let updated = text.replacen(find, replace, 1);
    if updated.len() > policy.output_limit.saturating_mul(16) {
        return Err(ToolError::Denied("result exceeds policy size limit".into()));
    }
    std::fs::write(&path, updated.as_bytes())?;
    let new_hash = hash_bytes(updated.as_bytes());
    Ok(ApplyResult {
        path: relative.display().to_string(),
        previous_hash,
        expected_hash: new_hash.clone(),
        new_hash,
    })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FetchResult {
    pub url: String,
    pub status: u16,
    pub content: String,
    pub truncated: bool,
    pub redacted: bool,
    pub artifact_hash: String,
}

/// Schemes a fetch may use. Anything else can reach the filesystem or a local
/// service without crossing the network the grant was given for.
const FETCH_SCHEMES: [&str; 2] = ["http", "https"];

/// Fetches one URL as text.
///
/// This is a fetch, not a search: there is no index and no query, so a caller
/// must already know the address. Naming it search would promise something it
/// does not do.
///
/// The result is untrusted input in the strongest sense — a remote party wrote
/// it — so it is bounded, redacted and hashed exactly like a file read, and it
/// grants nothing: a page saying to run a command is prose, and the command
/// still has to pass policy.
pub async fn fetch_url(policy: &ToolPolicy, url: &str) -> Result<FetchResult, ToolError> {
    policy.require(Approval::NetworkAccess)?;
    let parsed = url::Url::parse(url)
        .map_err(|_| ToolError::Denied("url is not a valid absolute URL".into()))?;
    if !FETCH_SCHEMES.contains(&parsed.scheme()) {
        return Err(ToolError::Denied(format!(
            "scheme {} is not fetchable; only http and https are",
            parsed.scheme()
        )));
    }
    let client = reqwest::Client::builder()
        .timeout(policy.timeout)
        // A redirect can change scheme or host after the check above, so the
        // check would be advisory rather than binding.
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| ToolError::Denied("could not build a fetch client".into()))?;
    let response = client.get(parsed.clone()).send().await.map_err(|error| {
        if error.is_timeout() {
            ToolError::Timeout
        } else {
            ToolError::Denied("fetch failed".into())
        }
    })?;
    let status = response.status().as_u16();
    let mut body = response.bytes_stream();
    let mut retained = Vec::with_capacity(policy.output_limit.min(64 * 1024));
    let mut hasher = blake3::Hasher::new();
    let mut observed = 0usize;
    use futures_util::StreamExt as _;
    while let Some(chunk) = body.next().await {
        let chunk = chunk.map_err(|error| {
            if error.is_timeout() {
                ToolError::Timeout
            } else {
                ToolError::Io(std::io::Error::other("fetch response body failed"))
            }
        })?;
        observed = observed.saturating_add(chunk.len());
        hasher.update(&chunk);
        let remaining = policy.output_limit.saturating_sub(retained.len());
        retained.extend_from_slice(&chunk[..chunk.len().min(remaining)]);
    }
    let truncated = observed > retained.len();
    let bounded = String::from_utf8_lossy(&retained);
    let (content, redacted) = policy.redact(&bounded);
    Ok(FetchResult {
        url: parsed.to_string(),
        status,
        artifact_hash: hasher.finalize().to_hex().to_string(),
        content,
        truncated,
        redacted,
    })
}

/// Creates a new file. Refuses to overwrite an existing one.
///
/// Creation and modification are separate capabilities on purpose: an edit
/// must carry the hash of what it replaces, and a create has nothing to hash.
/// Letting one tool do both would mean a blind overwrite is always one missing
/// argument away.
pub fn write_file(
    policy: &ToolPolicy,
    relative: &Path,
    content: &str,
) -> Result<ApplyResult, ToolError> {
    if let Some(approval) = edit_approval(relative) {
        policy.require(approval)?;
    }
    let path = policy.resolve(relative)?;
    if path.exists() {
        return Err(ToolError::Denied(
            "file exists; read it and use apply_replace with its hash".into(),
        ));
    }
    if content.len() > policy.output_limit {
        return Err(ToolError::Denied(
            "content exceeds policy size limit".into(),
        ));
    }
    // Parent directories are created only inside the workspace, since `resolve`
    // has already refused anything that escapes it.
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, content.as_bytes())?;
    let new_hash = hash_bytes(content.as_bytes());
    Ok(ApplyResult {
        path: relative.display().to_string(),
        previous_hash: String::new(),
        expected_hash: new_hash.clone(),
        new_hash,
    })
}

/// Reads a root-relative text file with a policy-derived byte bound.
pub fn read_file(policy: &ToolPolicy, relative: &Path) -> Result<FileReadResult, ToolError> {
    read_file_window(policy, relative, None, None)
}

/// Reads a window of a file, one-based and inclusive of `first_line`.
///
/// A file larger than the output bound was previously truncated mid-way with
/// nothing to say where the cut fell, so a caller could neither see the rest
/// nor ask for it. A window can be asked for, and the result says how many
/// lines the file has.
pub fn read_file_window(
    policy: &ToolPolicy,
    relative: &Path,
    first_line: Option<usize>,
    max_lines: Option<usize>,
) -> Result<FileReadResult, ToolError> {
    let path = policy.resolve(relative)?;
    let metadata = std::fs::metadata(&path)?;
    if !metadata.is_file() {
        return Err(ToolError::Denied(
            "read target is not a regular file".into(),
        ));
    }
    let first = first_line.unwrap_or(1).max(1);
    let last = max_lines
        .map(|count| first.saturating_add(count.saturating_sub(1)))
        .unwrap_or(usize::MAX);
    let mut file = std::fs::File::open(&path)?;
    let mut chunk = [0u8; 8192];
    let mut hasher = blake3::Hasher::new();
    let mut retained = Vec::with_capacity(policy.output_limit.min(64 * 1024));
    let mut selected_bytes = 0usize;
    let mut line = 1usize;
    let mut bytes_seen = 0usize;
    let mut last_byte = None;
    let mut binary = false;
    loop {
        let read = file.read(&mut chunk)?;
        if read == 0 {
            break;
        }
        hasher.update(&chunk[..read]);
        for byte in &chunk[..read] {
            if bytes_seen < 4096 && *byte == 0 {
                binary = true;
            }
            bytes_seen = bytes_seen.saturating_add(1);
            if line >= first && line <= last {
                selected_bytes = selected_bytes.saturating_add(1);
                if retained.len() < policy.output_limit {
                    retained.push(*byte);
                }
            }
            if *byte == b'\n' {
                line = line.saturating_add(1);
            }
            last_byte = Some(*byte);
        }
    }
    if binary {
        return Err(ToolError::Denied("binary file reads are denied".into()));
    }
    let total_lines = if bytes_seen == 0 {
        0
    } else {
        line.saturating_sub(usize::from(last_byte == Some(b'\n')))
    };
    if total_lines > 0 && first > total_lines {
        return Err(ToolError::Denied(format!(
            "first_line {first} is past the end of a {total_lines}-line file"
        )));
    }
    // `str::lines` does not include the selected window's terminal newline.
    if retained.last() == Some(&b'\n') && selected_bytes <= policy.output_limit {
        retained.pop();
        selected_bytes = selected_bytes.saturating_sub(1);
    }
    let bounded = selected_bytes > retained.len();
    let content = String::from_utf8_lossy(&retained).to_string();
    let (content, redacted) = policy.redact(&content);
    let artifact_hash = hasher.finalize().to_hex().to_string();
    Ok(FileReadResult {
        path: relative.display().to_string(),
        artifact_hash: artifact_hash.clone(),
        expected_hash: artifact_hash,
        content,
        truncated: bounded || first > 1 || last < total_lines,
        redacted,
        total_lines,
        first_line: first,
    })
}
/// Searches only text files below the policy root and returns a bounded match list.
pub fn search(
    policy: &ToolPolicy,
    query: &str,
    max_matches: usize,
) -> Result<Vec<SearchMatch>, ToolError> {
    if query.is_empty() {
        return Err(ToolError::Denied("empty search query".into()));
    }
    let deadline = std::time::Instant::now() + policy.timeout;
    let mut output = Vec::new();
    let mut output_bytes = 0usize;
    'files: for (path, directory) in walk_workspace(&policy.root)? {
        if std::time::Instant::now() >= deadline {
            return Err(ToolError::Timeout);
        }
        if output.len() >= max_matches || output_bytes >= policy.output_limit {
            break;
        }
        if directory {
            continue;
        }
        let metadata = std::fs::symlink_metadata(&path)?;
        if metadata.len() > MAX_SEARCHED_BYTES {
            continue;
        }
        let bytes = std::fs::read(&path)?;
        if bytes.iter().take(4096).any(|b| *b == 0) {
            continue;
        }
        let relative = path
            .strip_prefix(&policy.root)
            .unwrap_or(&path)
            .display()
            .to_string();
        for (number, line) in String::from_utf8_lossy(&bytes).lines().enumerate() {
            if !line.contains(query) {
                continue;
            }
            let remaining = policy
                .output_limit
                .saturating_sub(output_bytes)
                .saturating_sub(relative.len());
            if remaining == 0 {
                break 'files;
            }
            let raw: String = line.chars().take(remaining).collect();
            let (excerpt, redacted) = policy.redact(&raw);
            output_bytes = output_bytes
                .saturating_add(relative.len())
                .saturating_add(excerpt.len());
            output.push(SearchMatch {
                path: relative.clone(),
                line: number + 1,
                excerpt,
                redacted,
            });
            if output.len() >= max_matches {
                break 'files;
            }
        }
    }
    Ok(output)
}

/// What version control says about the workspace, structurally.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct VcsStatus {
    /// Absent where the workspace is not a checkout, rather than invented.
    pub branch: Option<String>,
    pub head: Option<String>,
    /// Paths changed since HEAD, with the two-letter porcelain code.
    pub changed: Vec<(String, String)>,
    pub truncated: bool,
}

/// Reads the working tree's status without changing anything.
///
/// The ledger names the files a session changed and their hashes, which
/// answers *what* and not *how much*. An agent could not see its own
/// accumulated change at all: it had to remember every file it had touched,
/// and a hash is not a diff.
///
/// Read-only by construction -- there is no argument here that reaches a
/// mutating subcommand, which is what keeps this out of the approval path that
/// `git clean` and `git push` sit behind.
pub async fn vcs_status(policy: &ToolPolicy) -> Result<VcsStatus, ToolError> {
    let porcelain = git_read(policy, &["status", "--porcelain=v1", "--branch"]).await?;
    let mut status = VcsStatus::default();
    for line in porcelain.lines() {
        if let Some(header) = line.strip_prefix("## ") {
            status.branch = header
                .split(['.', ' '])
                .next()
                .filter(|name| !name.is_empty() && *name != "HEAD")
                .map(str::to_string);
            continue;
        }
        if line.len() < 4 {
            continue;
        }
        let (code, path) = line.split_at(2);
        status.changed.push((
            code.trim().to_string(),
            path.trim().trim_matches('"').to_string(),
        ));
    }
    status.head = git_read(policy, &["rev-parse", "HEAD"])
        .await
        .ok()
        .map(|head| head.trim().to_string())
        .filter(|head| !head.is_empty());
    Ok(status)
}

/// The working tree's diff against HEAD, bounded like any other output.
///
/// `paths` narrows it, because a whole-repository diff is usually not the
/// question and always the largest possible answer.
pub async fn vcs_diff(policy: &ToolPolicy, paths: &[String]) -> Result<ToolResult, ToolError> {
    let mut args: Vec<String> = vec![
        "diff".into(),
        // A diff read by a model has no use for colour or pager control
        // sequences, and they are bytes off the output budget.
        "--no-color".into(),
        "--no-ext-diff".into(),
    ];
    if !paths.is_empty() {
        args.push("--".into());
        args.extend(paths.iter().cloned());
    }
    run_git(policy, &args).await
}

/// Runs a read-only git subcommand and returns its stdout.
async fn git_read(policy: &ToolPolicy, args: &[&str]) -> Result<String, ToolError> {
    let args: Vec<String> = args.iter().map(|arg| (*arg).to_string()).collect();
    let result = run_git(policy, &args).await?;
    if result.exit_code != Some(0) {
        return Err(ToolError::Denied(format!(
            "git {} failed: {}",
            args.join(" "),
            result.stderr.trim()
        )));
    }
    Ok(result.stdout)
}

/// Runs git under the tool policy, bypassing the derived command allowlist.
///
/// The allowlist is derived from what the repository declares as its checks,
/// and a repository does not declare `git` as a way of verifying itself. These
/// subcommands are read-only and fixed here rather than named by a caller, so
/// nothing the deployment sends can turn one into a mutation.
async fn run_git(policy: &ToolPolicy, args: &[String]) -> Result<ToolResult, ToolError> {
    let mut policy = policy.clone();
    if !policy.allow_commands.iter().any(|c| c == "git") {
        policy.allow_commands.push("git".into());
    }
    run_command(&policy, "git", args).await
}

pub async fn run_command(
    policy: &ToolPolicy,
    executable: &str,
    args: &[String],
) -> Result<ToolResult, ToolError> {
    run_command_with_stdin(policy, executable, args, None).await
}

/// The same, with text written to the command's standard input.
pub async fn run_command_with_stdin(
    policy: &ToolPolicy,
    executable: &str,
    args: &[String],
    stdin: Option<&str>,
) -> Result<ToolResult, ToolError> {
    // A program name never contains whitespace, so this is a whole command line
    // put where the executable belongs. Left to run, it reaches exec as one
    // filename and comes back as `execvp() of 'ls -la' failed: No such file or
    // directory`, which reads like a missing program rather than a malformed
    // call. Measured across several runs -- `ls -la`, and the same shape again
    // and again -- each costing an action to a message that did not say what
    // was wrong.
    if executable.split_whitespace().count() > 1 {
        let mut words = executable.split_whitespace();
        let program = words.next().unwrap_or_default();
        let rest: Vec<&str> = words.collect();
        return Err(ToolError::Denied(format!(
            "`{executable}` is a command line, not a program name. Pass the program alone as \
             executable -- `{program}` -- and put {} in args.",
            rest.join(" ")
        )));
    }
    // The derived allowlist cannot name the toolchain a workspace does not yet
    // have: a task that must install a JDK needs an executable no marker in the
    // repository could have implied. The grant is what widens it, and it is
    // recorded in the audit like every other.
    if !policy.approvals.contains(&Approval::ToolchainInstall)
        && !policy.allow_commands.iter().any(|x| x == executable)
    {
        return Err(ToolError::Denied(format!(
            "command {executable} is not allowlisted"
        )));
    }
    if !policy.network_allowed()
        && args
            .iter()
            .any(|a| a.contains("http://") || a.contains("https://"))
    {
        return Err(ToolError::Denied(
            "network access requires an explicit grant".into(),
        ));
    }
    if let Some(approval) = command_approval(executable, args) {
        policy.require(approval)?;
    }
    let sandboxed = policy.will_sandbox()?;
    let mut command = policy.prepare_command(executable, args)?;
    let started = std::time::Instant::now();
    command
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        // Dropping a cancelled poorAI future must not orphan the process.
        .kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);
    let mut child = command.spawn()?;
    let mut process_group = ProcessGroupGuard::new(child.id());
    if let Some(mut pipe) = child.stdin.take() {
        use tokio::io::AsyncWriteExt as _;
        let input = stdin.unwrap_or_default().as_bytes().to_vec();
        tokio::spawn(async move {
            let _ = pipe.write_all(&input).await;
            let _ = pipe.shutdown().await;
        });
    }
    let stdout_task = tokio::spawn(read_bounded_pipe(
        child
            .stdout
            .take()
            .ok_or_else(|| ToolError::Io(std::io::Error::other("child stdout was not piped")))?,
        policy.output_limit,
    ));
    let stderr_task = tokio::spawn(read_bounded_pipe(
        child
            .stderr
            .take()
            .ok_or_else(|| ToolError::Io(std::io::Error::other("child stderr was not piped")))?,
        policy.output_limit,
    ));
    let status = match timeout(policy.timeout, child.wait()).await {
        Ok(status) => status?,
        Err(_) => {
            process_group.terminate();
            let _ = child.kill().await;
            let _ = child.wait().await;
            return Err(ToolError::Timeout);
        }
    };
    process_group.disarm();
    let stdout_capture = stdout_task
        .await
        .map_err(|_| ToolError::Io(std::io::Error::other("stdout reader failed")))??;
    let stderr_capture = stderr_task
        .await
        .map_err(|_| ToolError::Io(std::io::Error::other("stderr reader failed")))??;
    let (stdout, a) = policy.redact(&String::from_utf8_lossy(&stdout_capture.retained));
    let (stderr, b) = policy.redact(&String::from_utf8_lossy(&stderr_capture.retained));
    let artifact_hash = hash_bytes(format!(
        "{}:{}:{:?}",
        stdout_capture.hash,
        stderr_capture.hash,
        status.code()
    ));
    Ok(ToolResult {
        exit_code: status.code(),
        artifact_hash,
        stdout,
        stderr,
        stdout_truncated: stdout_capture.truncated,
        stderr_truncated: stderr_capture.truncated,
        duration_ms: started.elapsed().as_millis(),
        redacted: a || b,
        sandboxed,
    })
}

struct ProcessGroupGuard {
    pid: Option<u32>,
}

impl ProcessGroupGuard {
    fn new(pid: Option<u32>) -> Self {
        Self { pid }
    }

    fn disarm(&mut self) {
        self.pid = None;
    }

    fn terminate(&mut self) {
        #[cfg(unix)]
        if let Some(pid) = self.pid.take() {
            let _ = std::process::Command::new("/bin/kill")
                .arg("-KILL")
                .arg(format!("-{pid}"))
                .status();
        }
        #[cfg(not(unix))]
        {
            self.pid = None;
        }
    }
}

impl Drop for ProcessGroupGuard {
    fn drop(&mut self) {
        self.terminate();
    }
}

struct BoundedPipe {
    retained: Vec<u8>,
    truncated: bool,
    hash: String,
}

async fn read_bounded_pipe(
    mut pipe: impl tokio::io::AsyncRead + Unpin,
    limit: usize,
) -> Result<BoundedPipe, std::io::Error> {
    use tokio::io::AsyncReadExt as _;
    let mut retained = Vec::with_capacity(limit.min(64 * 1024));
    let mut hasher = blake3::Hasher::new();
    let mut buffer = [0u8; 8192];
    let mut observed = 0usize;
    loop {
        let read = pipe.read(&mut buffer).await?;
        if read == 0 {
            break;
        }
        observed = observed.saturating_add(read);
        hasher.update(&buffer[..read]);
        let remaining = limit.saturating_sub(retained.len());
        retained.extend_from_slice(&buffer[..read.min(remaining)]);
    }
    Ok(BoundedPipe {
        truncated: observed > retained.len(),
        retained,
        hash: hasher.finalize().to_hex().to_string(),
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn traversal_denied() {
        let p = ToolPolicy {
            root: PathBuf::from("/tmp/root"),
            extra_readable: Vec::new(),
            allow_commands: vec![],
            output_limit: 1,
            timeout: Duration::from_secs(1),
            sandbox: SandboxPolicy::Disabled,
            approvals: Vec::new(),
        };
        assert!(p.resolve(Path::new("../secret")).is_err());
    }
    #[test]
    fn redacts() {
        let p = ToolPolicy {
            root: PathBuf::new(),
            extra_readable: Vec::new(),
            allow_commands: vec![],
            output_limit: 1,
            timeout: Duration::ZERO,
            sandbox: SandboxPolicy::Disabled,
            approvals: Vec::new(),
        };
        assert!(p.redact("token=abc").0.contains("REDACTED"));
    }
    #[test]
    fn read_rejects_binary_and_escapes() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("safe.txt"), "token=secret").unwrap();
        std::fs::write(root.path().join("bad.bin"), [0u8, 1]).unwrap();
        let policy = ToolPolicy {
            root: root.path().to_path_buf(),
            extra_readable: Vec::new(),
            allow_commands: vec![],
            output_limit: 100,
            timeout: Duration::ZERO,
            sandbox: SandboxPolicy::Disabled,
            approvals: Vec::new(),
        };
        assert!(read_file(&policy, Path::new("bad.bin")).is_err());
        assert!(read_file(&policy, Path::new("../safe.txt")).is_err());
        assert!(read_file(&policy, Path::new("safe.txt")).unwrap().redacted);
    }
    #[test]
    fn replacement_requires_fresh_hash() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("safe.txt"), "before").unwrap();
        let policy = ToolPolicy {
            root: root.path().to_path_buf(),
            extra_readable: Vec::new(),
            allow_commands: vec![],
            output_limit: 100,
            timeout: Duration::ZERO,
            sandbox: SandboxPolicy::Disabled,
            approvals: Vec::new(),
        };
        assert!(apply_replace(&policy, Path::new("safe.txt"), "wrong", "after").is_err());
        let hash = hash_bytes("before");
        assert_eq!(
            apply_replace(&policy, Path::new("safe.txt"), &hash, "after")
                .unwrap()
                .new_hash,
            hash_bytes("after")
        );
    }
    #[cfg(unix)]
    #[test]
    fn symlink_escape_is_denied() {
        use std::os::unix::fs::symlink;
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        symlink(outside.path(), root.path().join("escape")).unwrap();
        let policy = ToolPolicy {
            root: root.path().to_path_buf(),
            extra_readable: Vec::new(),
            allow_commands: vec![],
            output_limit: 100,
            timeout: Duration::ZERO,
            sandbox: SandboxPolicy::Disabled,
            approvals: Vec::new(),
        };
        assert!(policy.resolve(Path::new("escape/secret.txt")).is_err());
    }
    #[cfg(unix)]
    #[test]
    fn tree_and_search_do_not_follow_symlinks() {
        use std::os::unix::fs::symlink;
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret.txt"), "needle").unwrap();
        symlink(outside.path(), root.path().join("escape")).unwrap();
        let policy = ToolPolicy {
            root: root.path().to_path_buf(),
            extra_readable: Vec::new(),
            allow_commands: vec![],
            output_limit: 100,
            timeout: Duration::from_secs(1),
            sandbox: SandboxPolicy::Disabled,
            approvals: Vec::new(),
        };
        assert!(list_tree(&policy, 10).unwrap().is_empty());
        assert!(search(&policy, "needle", 10).unwrap().is_empty());
    }
    #[test]
    fn development_profile_still_denies_network() {
        let policy = PolicyProfile::Development.build(PathBuf::from("/tmp"));
        assert!(!policy.network_allowed());
        assert!(policy.allow_commands.contains(&"cargo".into()));
    }
}
