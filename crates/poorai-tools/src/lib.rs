//! Typed, bounded workspace tools and policy enforcement.
use poorai_domain::hash_bytes;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::{
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
pub enum ToolCapability {
    ReadFile,
    Search,
    ListTree,
    ApplyPatch,
    GitDiff,
    RunCommand,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "capability", rename_all = "snake_case")]
pub enum ActionProposal {
    Complete {
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
    },
    FetchUrl {
        url: String,
    },
}
impl ActionProposal {
    pub fn validate(&self) -> Result<(), ToolError> {
        match self {
            Self::Complete { rationale } if rationale.is_empty() => {
                Err(ToolError::Denied("completion rationale is required".into()))
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
    /// Rewriting version-control history.
    HistoryRewrite,
    /// Publishing a package or pushing to a remote.
    Publish,
    /// Reaching the network at all, for the workspace and for any process it
    /// runs. Dependency resolution needs it; so does exfiltration.
    NetworkAccess,
}

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
        // `git reset --hard` and `git clean -fd` discard uncommitted work.
        if args.iter().any(|arg| arg == "reset") && args.iter().any(|arg| arg == "--hard") {
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
        ActionProposal::RunCommand { executable, args } => command_approval(executable, args)
            .map(|approval| (approval, format!("run `{executable} {}`", args.join(" ")))),
        ActionProposal::ApplyReplace { path, .. } | ActionProposal::WriteFile { path, .. } => {
            edit_approval(Path::new(path)).map(|a| (a, format!("write {path}")))
        }
        ActionProposal::ReplaceText { path, find, .. } => edit_approval(Path::new(path))
            .map(|a| (a, format!("change {path} where it reads `{}`", elide(find)))),
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
        let network = if self.network_allowed() {
            ""
        } else {
            "(deny network*)"
        };
        Some(format!(
            "(version 1)(allow default)(deny file-write*)(allow file-write* (subpath \"{root}\"))(allow file-write-data (literal \"/dev/null\") (literal \"/dev/stdout\") (literal \"/dev/stderr\")){network}"
        ))
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
pub fn list_tree(policy: &ToolPolicy, max_entries: usize) -> Result<Vec<TreeEntry>, ToolError> {
    let mut output = Vec::new();
    list_dir(&policy.root, &policy.root, max_entries, &mut output)?;
    Ok(output)
}
fn list_dir(
    root: &Path,
    directory: &Path,
    max: usize,
    output: &mut Vec<TreeEntry>,
) -> Result<(), ToolError> {
    for entry in std::fs::read_dir(directory)? {
        if output.len() >= max {
            break;
        }
        let entry = entry?;
        let path = entry.path();
        let name = entry.file_name();
        if [".git", "target", "node_modules", ".poorai"]
            .iter()
            .any(|blocked| name == *blocked)
        {
            continue;
        }
        let metadata = std::fs::symlink_metadata(&path)?;
        if metadata.file_type().is_symlink() {
            continue;
        }
        let directory = metadata.is_dir();
        output.push(TreeEntry {
            path: path
                .strip_prefix(root)
                .unwrap_or(&path)
                .display()
                .to_string(),
            directory,
        });
        if directory {
            list_dir(root, &path, max, output)?;
        }
    }
    Ok(())
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
    let body = response
        .text()
        .await
        .map_err(|_| ToolError::Denied("response was not readable text".into()))?;
    let truncated = body.len() > policy.output_limit;
    let bounded: String = body.chars().take(policy.output_limit).collect();
    let (content, redacted) = policy.redact(&bounded);
    Ok(FetchResult {
        url: parsed.to_string(),
        status,
        artifact_hash: hash_bytes(body.as_bytes()),
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
    let bytes = std::fs::read(&path)?;
    if bytes.iter().take(4096).any(|b| *b == 0) {
        return Err(ToolError::Denied("binary file reads are denied".into()));
    }
    let whole = String::from_utf8_lossy(&bytes);
    let lines: Vec<&str> = whole.lines().collect();
    let total_lines = lines.len();
    let first = first_line.unwrap_or(1).max(1);
    if total_lines > 0 && first > total_lines {
        return Err(ToolError::Denied(format!(
            "first_line {first} is past the end of a {total_lines}-line file"
        )));
    }
    let window: Vec<&str> = lines
        .iter()
        .skip(first - 1)
        .take(max_lines.unwrap_or(usize::MAX))
        .copied()
        .collect();
    let selected = window.join("\n");
    let bounded = selected.len() > policy.output_limit;
    let content: String = if bounded {
        selected.chars().take(policy.output_limit).collect()
    } else {
        selected
    };
    let (content, redacted) = policy.redact(&content);
    Ok(FileReadResult {
        path: relative.display().to_string(),
        artifact_hash: hash_bytes(&bytes),
        expected_hash: hash_bytes(&bytes),
        content,
        truncated: bounded || first > 1 || window.len() < total_lines,
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
    let mut output = Vec::new();
    search_dir(policy, &policy.root, query, max_matches, &mut output)?;
    Ok(output)
}
fn search_dir(
    policy: &ToolPolicy,
    directory: &Path,
    query: &str,
    max: usize,
    output: &mut Vec<SearchMatch>,
) -> Result<(), ToolError> {
    for entry in std::fs::read_dir(directory)? {
        if output.len() >= max {
            break;
        }
        let entry = entry?;
        let path = entry.path();
        let name = entry.file_name();
        if [".git", "target", "node_modules", ".poorai"]
            .iter()
            .any(|blocked| name == *blocked)
        {
            continue;
        }
        let metadata = std::fs::symlink_metadata(&path)?;
        if metadata.file_type().is_symlink() {
            continue;
        }
        if metadata.is_dir() {
            search_dir(policy, &path, query, max, output)?;
        } else if path.is_file() {
            let bytes = std::fs::read(&path)?;
            if bytes.len() > 1_000_000 || bytes.iter().take(4096).any(|b| *b == 0) {
                continue;
            }
            for (number, line) in String::from_utf8_lossy(&bytes).lines().enumerate() {
                if line.contains(query) {
                    let (excerpt, redacted) =
                        policy.redact(&line.chars().take(policy.output_limit).collect::<String>());
                    output.push(SearchMatch {
                        path: path
                            .strip_prefix(&policy.root)
                            .unwrap_or(&path)
                            .display()
                            .to_string(),
                        line: number + 1,
                        excerpt,
                        redacted,
                    });
                    if output.len() >= max {
                        break;
                    }
                }
            }
        }
    }
    Ok(())
}
pub async fn run_command(
    policy: &ToolPolicy,
    executable: &str,
    args: &[String],
) -> Result<ToolResult, ToolError> {
    if !policy.allow_commands.iter().any(|x| x == executable) {
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
    let profile = match policy.sandbox {
        SandboxPolicy::Disabled => None,
        SandboxPolicy::Preferred | SandboxPolicy::Required => policy.sandbox_profile(),
    };
    if profile.is_none() && policy.sandbox == SandboxPolicy::Required {
        return Err(ToolError::Denied(
            "policy requires a sandbox and this platform provides none".into(),
        ));
    }
    let sandboxed = profile.is_some();
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
    // doctest run -- and the system one lies outside the sandbox. Rather than
    // widening the boundary to all of $TMPDIR, which would let one workspace
    // write into another's, the child is given a scratch directory inside its
    // own root.
    // The canonical root, for the same reason the seatbelt profile needs it: on
    // macOS the uncanonical path is a symlink, and handing the child that path
    // makes its scratch directory look like it lies outside the workspace.
    let scratch = policy
        .root
        .canonicalize()
        .unwrap_or_else(|_| policy.root.clone())
        .join(SCRATCH_DIRECTORY);
    let _ = std::fs::create_dir_all(&scratch);
    let started = std::time::Instant::now();
    let output = timeout(
        policy.timeout,
        command
            .current_dir(&policy.root)
            .env_clear()
            // PATH is explicitly allowlisted solely for executable resolution; it is never logged.
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("TMPDIR", &scratch)
            .env("TMP", &scratch)
            .env("TEMP", &scratch)
            // Package managers keep caches and config under HOME. Pointing it
            // into the workspace keeps them inside the boundary instead of
            // widening it, and makes a run hermetic: nothing it downloads
            // persists into the next one, and nothing in the real home
            // directory is read. npm reports the denial as a permissions bug
            // in its own cache, which is worth knowing when diagnosing one.
            .env("HOME", &scratch)
            .output(),
    )
    .await
    .map_err(|_| ToolError::Timeout)??;
    let cap = |s: Vec<u8>| {
        String::from_utf8_lossy(&s)
            .chars()
            .take(policy.output_limit)
            .collect::<String>()
    };
    let (stdout, a) = policy.redact(&cap(output.stdout));
    let (stderr, b) = policy.redact(&cap(output.stderr));
    Ok(ToolResult {
        exit_code: output.status.code(),
        artifact_hash: hash_bytes(format!("{stdout}{stderr}")),
        stdout,
        stderr,
        duration_ms: started.elapsed().as_millis(),
        redacted: a || b,
        sandboxed,
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn traversal_denied() {
        let p = ToolPolicy {
            root: PathBuf::from("/tmp/root"),
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
            allow_commands: vec![],
            output_limit: 100,
            timeout: Duration::ZERO,
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
