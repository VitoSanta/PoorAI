//! Frozen evaluation corpus: types, loading, materialisation and scoring.
//!
//! The corpus is data, not code. A suite is loaded, hashed and materialised
//! into a throwaway workspace per task, so a run is reproducible from the
//! corpus revision alone.

use poorai_domain::hash_bytes;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Debug, thiserror::Error)]
pub enum EvalError {
    #[error("corpus I/O failure: {0}")]
    Io(#[from] std::io::Error),
    #[error("corpus is invalid: {0}")]
    Invalid(String),
}

/// What a task exercises. Recorded so results can be read per category rather
/// than only in aggregate — a suite that passes only its easiest category is
/// not a suite that passed.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum TaskKind {
    Bugfix,
    MultiFile,
    RepositoryQuestion,
    Refactor,
    TestFailure,
    PolicyAttack,
    /// Build something that does not exist yet. Scored on what the workspace
    /// does afterwards, not on which files were touched: the agent chooses the
    /// structure, so an allowed-file list would be scoring a style.
    Generation,
}

/// A command the harness runs against a task's workspace.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Verifier {
    pub executable: String,
    #[serde(default)]
    pub args: Vec<String>,
}

/// A real project a task is set in, pinned so the workspace is the same every
/// time it is materialised.
///
/// A commit id is a content address: the tree it names cannot change under us,
/// which is what makes an external repository as reproducible as an inlined
/// one. The recorded `tree_hash` is checked after checkout anyway, because a
/// silent mismatch would mean the run was measured against a workspace nobody
/// declared.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RepositorySource {
    pub url: String,
    /// The commit the defect is present at -- in practice the parent of the
    /// fix, so the repository is exactly as it was before anyone repaired it.
    pub commit: String,
    /// The upstream fix. Recorded for provenance rather than used: it says
    /// where the hidden test came from, and its date is what a reader needs in
    /// order to judge whether a deployment could have memorised the answer.
    pub fix_commit: String,
    pub fix_committed_at: String,
    /// Git tree hash at `commit`, verified after checkout.
    pub tree_hash: String,
    /// Commands run before the agent starts, outside the sandbox and with the
    /// network, to install what the project's own tests need.
    ///
    /// Outside, deliberately: preparing a workspace is the harness's work, and
    /// the agent that is measured never has the network the preparation used.
    #[serde(default)]
    pub setup: Vec<Verifier>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Task {
    pub id: String,
    pub kind: TaskKind,
    pub statement: String,
    /// Workspace-relative paths the task is allowed to change. A change
    /// anywhere else is a violation, not a stylistic difference.
    pub allowed_files: Vec<String>,
    /// The initial workspace, path to contents.
    ///
    /// Empty when the task draws its workspace from `repository` instead: a
    /// real project is tens of megabytes of tree, and inlining one would make
    /// the corpus unreadable without making it more reproducible than a pinned
    /// commit already does.
    #[serde(default)]
    pub files: BTreeMap<String, String>,
    /// An external repository this task is set in, checked out before the run.
    #[serde(default)]
    pub repository: Option<RepositorySource>,
    /// The check the agent can see and run itself.
    pub visible_verifier: Verifier,
    /// The check the agent never sees, run by the harness afterwards.
    ///
    /// A visible test can be satisfied by editing the test. A hidden one
    /// cannot, so the pair separates a repair from a defeat of the check.
    pub hidden_verifier: Verifier,
    /// Files written only after the agent has finished, immediately before the
    /// hidden verifier runs. The agent cannot read, edit or anticipate them.
    #[serde(default)]
    pub hidden_files: BTreeMap<String, String>,
    /// For a `RepositoryQuestion`, a string the completion rationale must
    /// contain. These tasks are answered rather than edited.
    #[serde(default)]
    pub expected_in_rationale: Option<String>,
    /// Files a `Generation` task must leave untouched — the specification it
    /// was given, and anything else that would let it rewrite its own target.
    #[serde(default)]
    pub protected_files: Vec<String>,
    /// Actions this task is allowed, overriding the execution profile. A task
    /// that builds several files needs more turns than one that edits a line,
    /// and the budget belongs with the task rather than with the deployment.
    #[serde(default)]
    pub max_actions: Option<u8>,
    /// Approvals this task grants. Recorded in the corpus so a result cannot
    /// be read without seeing what the agent was permitted.
    #[serde(default)]
    pub approvals: Vec<String>,
    pub time_budget_secs: u64,
    /// Where the task came from, so contamination can be reasoned about.
    pub provenance: String,
    /// For `PolicyAttack`, the task passes when the agent does NOT do this.
    #[serde(default)]
    pub must_not_happen: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Suite {
    pub name: String,
    pub tasks: Vec<Task>,
}

impl Suite {
    /// Content hash of the whole suite. Two runs with different revisions are
    /// not comparable, and this is what makes that detectable.
    pub fn revision(&self) -> String {
        hash_bytes(serde_json::to_vec(&self.tasks).expect("suite is serializable"))
    }

    pub fn load(path: &Path) -> Result<Self, EvalError> {
        let suite: Suite = serde_json::from_slice(&std::fs::read(path)?)
            .map_err(|e| EvalError::Invalid(e.to_string()))?;
        suite.validate()?;
        Ok(suite)
    }

    fn validate(&self) -> Result<(), EvalError> {
        if self.tasks.is_empty() {
            return Err(EvalError::Invalid("suite has no tasks".into()));
        }
        let mut seen = std::collections::BTreeSet::new();
        for task in &self.tasks {
            if !seen.insert(&task.id) {
                return Err(EvalError::Invalid(format!("duplicate task id {}", task.id)));
            }
            if task.files.is_empty() && task.repository.is_none() {
                return Err(EvalError::Invalid(format!(
                    "task {} has neither files nor a repository",
                    task.id
                )));
            }
            if !task.files.is_empty() && task.repository.is_some() {
                return Err(EvalError::Invalid(format!(
                    "task {} declares both inline files and a repository; \
                     which one is the workspace would be ambiguous",
                    task.id
                )));
            }
            if task.provenance.trim().is_empty() {
                return Err(EvalError::Invalid(format!(
                    "task {} has no provenance note",
                    task.id
                )));
            }
            for allowed in task
                .allowed_files
                .iter()
                .filter(|_| task.repository.is_none())
            {
                if !task.files.contains_key(allowed) {
                    return Err(EvalError::Invalid(format!(
                        "task {} allows {allowed}, which is not in its workspace",
                        task.id
                    )));
                }
            }
            if task.kind == TaskKind::RepositoryQuestion && task.expected_in_rationale.is_none() {
                return Err(EvalError::Invalid(format!(
                    "repository question {} declares no expected answer",
                    task.id
                )));
            }
            // A hidden file the agent could also see would not be hidden.
            if let Some(clash) = task
                .hidden_files
                .keys()
                .find(|path| task.files.contains_key(*path))
            {
                return Err(EvalError::Invalid(format!(
                    "task {} hides {clash}, which is already in its visible workspace",
                    task.id
                )));
            }
            if task.kind == TaskKind::Generation && task.protected_files.is_empty() {
                return Err(EvalError::Invalid(format!(
                    "generation task {} protects no file, so nothing stops it rewriting its own target",
                    task.id
                )));
            }
            for protected in task
                .protected_files
                .iter()
                .filter(|_| task.repository.is_none())
            {
                if !task.files.contains_key(protected) {
                    return Err(EvalError::Invalid(format!(
                        "task {} protects {protected}, which is not in its workspace",
                        task.id
                    )));
                }
            }
            if task.kind == TaskKind::PolicyAttack && task.must_not_happen.is_none() {
                return Err(EvalError::Invalid(format!(
                    "policy attack {} does not say what must not happen",
                    task.id
                )));
            }
            // A path that escapes the workspace would write outside the
            // sandbox when materialised.
            for path in task.files.keys().chain(task.hidden_files.keys()) {
                if Path::new(path).is_absolute()
                    || Path::new(path)
                        .components()
                        .any(|c| matches!(c, std::path::Component::ParentDir))
                {
                    return Err(EvalError::Invalid(format!(
                        "task {} has an escaping path {path}",
                        task.id
                    )));
                }
            }
        }
        Ok(())
    }
}

/// What checking one external task against its own repository established.
///
/// A corpus task is only worth running if the defect it names is really
/// present and the hidden test really discriminates. Both are properties of
/// the upstream commits, not of anything poorAI wrote, so both can be checked
/// rather than asserted.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExternalTaskCheck {
    pub task_id: String,
    /// The project's own suite passes at the commit the task starts from. A
    /// workspace that is already broken would score every deployment as
    /// failing for reasons that have nothing to do with the task.
    pub visible_passes_at_start: bool,
    /// The hidden test fails at that commit -- the defect is present.
    pub hidden_fails_at_start: bool,
    /// The hidden test passes at the upstream fix -- the task is solvable, and
    /// the test is measuring the thing the fix changed.
    pub hidden_passes_at_fix: bool,
    pub detail: String,
}

impl ExternalTaskCheck {
    pub fn sound(&self) -> bool {
        self.visible_passes_at_start && self.hidden_fails_at_start && self.hidden_passes_at_fix
    }
}

/// Checks that an external task is fair before any deployment is measured on it.
pub fn check_external_task(task: &Task) -> Result<ExternalTaskCheck, EvalError> {
    let Some(source) = &task.repository else {
        return Err(EvalError::Invalid(format!(
            "task {} is not set in an external repository",
            task.id
        )));
    };
    let run = |root: &Path, verifier: &Verifier| -> bool {
        std::process::Command::new(&verifier.executable)
            .args(&verifier.args)
            .current_dir(root)
            .output()
            .map(|output| output.status.success())
            .unwrap_or(false)
    };

    let start = tempfile::tempdir()?;
    materialise_repository(source, start.path())?;
    let visible_passes_at_start = run(start.path(), &task.visible_verifier);
    materialise_hidden(task, start.path())?;
    let hidden_fails_at_start = !run(start.path(), &task.hidden_verifier);

    // The same repository at the upstream fix. Its tree is whatever that
    // commit produced, so no tree hash is declared for it and none is checked.
    let fixed = tempfile::tempdir()?;
    let at_fix = RepositorySource {
        commit: source.fix_commit.clone(),
        tree_hash: String::new(),
        ..source.clone()
    };
    materialise_repository_unchecked(&at_fix, fixed.path())?;
    materialise_hidden(task, fixed.path())?;
    let hidden_passes_at_fix = run(fixed.path(), &task.hidden_verifier);

    Ok(ExternalTaskCheck {
        task_id: task.id.clone(),
        visible_passes_at_start,
        hidden_fails_at_start,
        hidden_passes_at_fix,
        detail: format!(
            "{} at {} (fix {} of {})",
            source.url, source.commit, source.fix_commit, source.fix_committed_at
        ),
    })
}

/// Writes the hidden files, immediately before the hidden verifier runs.
pub fn materialise_hidden(task: &Task, root: &Path) -> Result<(), EvalError> {
    write_all(&task.hidden_files, root)
}

/// Writes a task's initial workspace into `root`.
///
/// Either the inline files, or a checkout of the repository the task pins.
pub fn materialise(task: &Task, root: &Path) -> Result<(), EvalError> {
    match &task.repository {
        Some(source) => materialise_repository(source, root),
        None => write_all(&task.files, root),
    }
}

/// Checks out a pinned commit into `root` and verifies the tree it produced.
///
/// The clone is shallow at one commit: a project's whole history is not the
/// workspace, and fetching it would make every run pay for it.
pub fn materialise_repository(source: &RepositorySource, root: &Path) -> Result<(), EvalError> {
    if source.tree_hash.is_empty() {
        return Err(EvalError::Invalid(format!(
            "{} at {} declares no tree hash, so the checkout could not be verified",
            source.url, source.commit
        )));
    }
    materialise_repository_unchecked(source, root)
}

/// The same, without requiring a declared tree hash. Used only to visit the
/// upstream fix while checking a task, where the tree is whatever that commit
/// produced and there is nothing to declare it against.
fn materialise_repository_unchecked(
    source: &RepositorySource,
    root: &Path,
) -> Result<(), EvalError> {
    let git = |args: &[&str]| -> Result<String, EvalError> {
        let output = std::process::Command::new("git")
            .args(args)
            .current_dir(root)
            .output()?;
        if !output.status.success() {
            return Err(EvalError::Invalid(format!(
                "git {} failed: {}",
                args.join(" "),
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    };
    std::fs::create_dir_all(root)?;
    git(&["init", "-q"])?;
    git(&["remote", "add", "origin", &source.url])?;
    git(&["fetch", "-q", "--depth", "1", "origin", &source.commit])?;
    git(&["checkout", "-q", "FETCH_HEAD"])?;

    // A commit id is a content address, so this should never differ. It is
    // checked because if it ever did, the run would have been measured against
    // a workspace nobody declared, and silence about that is the one outcome
    // worth ruling out.
    let tree = git(&["rev-parse", "HEAD^{tree}"])?;
    if !source.tree_hash.is_empty() && tree != source.tree_hash {
        return Err(EvalError::Invalid(format!(
            "checkout of {} at {} produced tree {tree}, but the corpus declares {}",
            source.url, source.commit, source.tree_hash
        )));
    }
    // The repository's own history is not part of the task, and leaving it
    // would let an agent read the fix out of the log.
    std::fs::remove_dir_all(root.join(".git"))?;
    for step in &source.setup {
        let output = std::process::Command::new(&step.executable)
            .args(&step.args)
            .current_dir(root)
            .output()?;
        if !output.status.success() {
            return Err(EvalError::Invalid(format!(
                "setup step `{} {}` failed: {}",
                step.executable,
                step.args.join(" "),
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
    }
    Ok(())
}

fn write_all(files: &BTreeMap<String, String>, root: &Path) -> Result<(), EvalError> {
    for (relative, contents) in files {
        let path = root.join(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, contents)?;
    }
    Ok(())
}

/// Files that differ from the task's initial workspace.
///
/// The tool scratch directory and agent state are harness artifacts, not task
/// changes, and are not scored.
/// Directories that belong to the harness or a package manager, not the task.
const UNSCORED_DIRECTORIES: [&str; 6] = [
    ".poorai",
    ".poorai-scratch",
    "node_modules",
    "target",
    ".git",
    "dist",
];

/// Every scored file in the workspace, with its content hash.
///
/// Taken after the repository's own checks have run once, so build artifacts
/// they generate — a lockfile, a compiled index — belong to the baseline
/// rather than being attributed to the agent. Excluding such files by name
/// would need a list per ecosystem and would be wrong the first time one was
/// missing.
pub fn snapshot(root: &Path) -> Result<BTreeMap<String, String>, EvalError> {
    let mut files = BTreeMap::new();
    collect_snapshot(root, root, &mut files)?;
    Ok(files)
}

fn collect_snapshot(
    root: &Path,
    directory: &Path,
    files: &mut BTreeMap<String, String>,
) -> Result<(), EvalError> {
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        let name = entry.file_name();
        if UNSCORED_DIRECTORIES.iter().any(|d| name == *d) {
            continue;
        }
        let path = entry.path();
        let metadata = std::fs::symlink_metadata(&path)?;
        if metadata.file_type().is_symlink() {
            continue;
        }
        if metadata.is_dir() {
            collect_snapshot(root, &path, files)?;
        } else {
            let relative = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .display()
                .to_string();
            files.insert(relative, hash_bytes(std::fs::read(&path)?));
        }
    }
    Ok(())
}

/// Files that differ from a snapshot, modified or created.
pub fn changed_since(
    before: &BTreeMap<String, String>,
    root: &Path,
) -> Result<Vec<String>, EvalError> {
    let now = snapshot(root)?;
    let mut changed: Vec<String> = now
        .iter()
        .filter(|(path, hash)| before.get(*path) != Some(hash))
        .map(|(path, _)| path.clone())
        .collect();
    // A file the agent deleted is a change too.
    changed.extend(
        before
            .keys()
            .filter(|path| !now.contains_key(*path))
            .cloned(),
    );
    changed.sort();
    changed.dedup();
    Ok(changed)
}

pub fn changed_files(task: &Task, root: &Path) -> Result<Vec<String>, EvalError> {
    let mut changed = Vec::new();
    for (relative, original) in &task.files {
        let current = std::fs::read_to_string(root.join(relative)).unwrap_or_default();
        if &current != original {
            changed.push(relative.clone());
        }
    }
    // Files the agent created are changes too. A generation task produces
    // nothing but created files, so a walk that only compared known paths
    // would score every one of them as having changed nothing.
    collect_created(task, root, root, &mut changed)?;
    changed.sort();
    changed.dedup();
    Ok(changed)
}

fn collect_created(
    task: &Task,
    root: &Path,
    directory: &Path,
    changed: &mut Vec<String>,
) -> Result<(), EvalError> {
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        let name = entry.file_name();
        if UNSCORED_DIRECTORIES.iter().any(|d| name == *d) {
            continue;
        }
        let path = entry.path();
        let metadata = std::fs::symlink_metadata(&path)?;
        if metadata.file_type().is_symlink() {
            continue;
        }
        if metadata.is_dir() {
            collect_created(task, root, &path, changed)?;
        } else {
            let relative = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .display()
                .to_string();
            if !task.files.contains_key(&relative) && !task.hidden_files.contains_key(&relative) {
                changed.push(relative);
            }
        }
    }
    Ok(())
}

/// Files the agent wrote that the task did not permit. This is a scoring
/// signal, not a policy one: policy already confines writes to the workspace,
/// while this asks whether the agent stayed inside the part of it the task
/// named.
///
/// `edited` is what the agent wrote through a tool, taken from the audit, and
/// is intersected with the filesystem diff rather than replacing it. A build
/// artefact is not an edit: three runs on more-itertools were scored as having
/// gone out of scope because editing `more.py` and running the project's own
/// tests regenerated `__pycache__/*.pyc`, which the interpreter wrote and the
/// deployment never touched. Deriving this from the diff alone cannot tell
/// those apart without a list of generated-file conventions, which would be
/// wrong for the next language as surely as the last such list was.
pub fn out_of_scope_changes(task: &Task, changed: &[String], edited: &[String]) -> Vec<String> {
    let by_the_agent = |path: &String| changed.contains(path) && edited.contains(path);
    match task.kind {
        // A generation task chooses its own structure, so only the files it was
        // told not to touch are out of scope.
        TaskKind::Generation => task
            .protected_files
            .iter()
            .filter(|path| by_the_agent(path))
            .cloned()
            .collect(),
        _ => edited
            .iter()
            .filter(|path| by_the_agent(path) && !task.allowed_files.contains(path))
            .cloned()
            .collect(),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskOutcome {
    pub task_id: String,
    pub kind: TaskKind,
    pub seed: u64,
    /// The agent declared completion and the loop's own verification passed.
    pub declared_complete: bool,
    /// The hidden verifier passed afterwards.
    pub hidden_verifier_passed: bool,
    pub visible_verifier_passed: bool,
    pub changed_files: Vec<String>,
    pub out_of_scope_changes: Vec<String>,
    pub tool_attempts: usize,
    pub tool_denials: usize,
    pub tool_failures: usize,
    pub duration_secs: f64,
    pub timed_out: bool,
    pub error: Option<String>,
    /// For a policy attack, whether the thing that must not happen did.
    pub violation: Option<String>,
    /// For a repository question, whether the rationale carried the answer.
    #[serde(default)]
    pub answer_matched: Option<bool>,
    /// The run ended because the provider or backend failed, not because the
    /// deployment could not do the task.
    #[serde(default)]
    pub provider_failure: bool,
    /// How many of each event the run produced.
    ///
    /// The workspace is thrown away, so without this a report cannot say
    /// whether the history was compacted, whether a plan was made, or whether
    /// a loop was named -- and those become things to infer rather than read.
    #[serde(default)]
    pub events: BTreeMap<String, usize>,
}

impl TaskOutcome {
    /// A task is resolved when the agent declared it done, the loop verified
    /// it, the hidden check agrees, and nothing outside the allowed files
    /// changed.
    ///
    /// The hidden check is what separates a repair from a defeat of the visible
    /// test: an agent that edits the test to make it pass satisfies the visible
    /// verifier and fails here.
    /// Whether this run measured the deployment at all.
    pub fn measured(&self) -> bool {
        !self.provider_failure
    }

    pub fn resolved(&self) -> bool {
        match self.kind {
            // An attack task is resolved by nothing bad happening.
            TaskKind::PolicyAttack => self.violation.is_none(),
            // Scored on what the workspace does, not on whether the agent
            // said it was finished: a generated app either serves the
            // contract or it does not.
            TaskKind::Generation => {
                self.hidden_verifier_passed && self.out_of_scope_changes.is_empty()
            }
            // A question is answered, not edited: the rationale must carry the
            // answer and the workspace must be untouched.
            TaskKind::RepositoryQuestion => {
                self.declared_complete
                    && self.answer_matched == Some(true)
                    && self.changed_files.is_empty()
            }
            _ => {
                self.declared_complete
                    && self.hidden_verifier_passed
                    && self.out_of_scope_changes.is_empty()
            }
        }
    }
}

/// Wilson score interval for a proportion, reported instead of a bare rate.
///
/// A rate of 3/6 and a rate of 300/600 are the same number and not the same
/// evidence; the interval is what says so.
pub fn wilson_interval(successes: usize, total: usize, z: f64) -> (f64, f64) {
    if total == 0 {
        return (0.0, 1.0);
    }
    let n = total as f64;
    let phat = successes as f64 / n;
    let denominator = 1.0 + z * z / n;
    let centre = phat + z * z / (2.0 * n);
    let spread = z * ((phat * (1.0 - phat) + z * z / (4.0 * n)) / n).sqrt();
    (
        ((centre - spread) / denominator).max(0.0),
        ((centre + spread) / denominator).min(1.0),
    )
}

/// 95% two-sided.
pub const Z_95: f64 = 1.959_964;

/// Whether a measured metric clears a predeclared bar.
///
/// Judged on the interval rather than the point estimate. A rate of 5/8 and a
/// rate of 500/800 are the same number and not the same evidence, so a bar can
/// only be called met when the evidence excludes being below it.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    /// The whole interval is above the bar.
    Met,
    /// The whole interval is below it.
    Failed,
    /// The interval spans the bar: more trials, not a decision.
    Inconclusive,
}

/// Judges a metric against a minimum bar.
pub fn verdict_at_least(metric: &Metric, bar: f64) -> Verdict {
    if metric.interval_low >= bar {
        Verdict::Met
    } else if metric.interval_high < bar {
        Verdict::Failed
    } else {
        Verdict::Inconclusive
    }
}

/// The upper bound a run of clean trials places on an unobserved failure rate.
///
/// A safety threshold of zero cannot be *met* by sampling: no finite number of
/// clean runs proves a rate is zero. It can only be falsified by one
/// occurrence, or left standing with a bound. Reporting "zero violations, so
/// the threshold is met" claims evidence the trials do not contain; reporting
/// "none observed in 24 runs, rate at most 0.138" states what they do.
pub fn unobserved_rate_bound(clean_runs: usize) -> f64 {
    wilson_interval(0, clean_runs, Z_95).1
}

/// Judges a metric against a maximum bar, for rates that must stay low.
pub fn verdict_at_most(metric: &Metric, bar: f64) -> Verdict {
    if metric.interval_high <= bar {
        Verdict::Met
    } else if metric.interval_low > bar {
        Verdict::Failed
    } else {
        Verdict::Inconclusive
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SuiteReport {
    pub suite: String,
    pub corpus_rev: String,
    pub harness_rev: String,
    pub model_digest: String,
    pub deployment_fingerprint: String,
    pub hardware_compatibility_key: String,
    pub execution_profile_id: poorai_domain::Id,
    pub seeds: Vec<u64>,
    /// Every sampling parameter actually in force, with where it came from.
    ///
    /// A report that names a seed and omits the temperature describes a
    /// reproducibility it does not have, and two runs cannot be compared
    /// without knowing whether a value was recommended, inherited or chosen.
    #[serde(default)]
    pub sampling: BTreeMap<String, poorai_domain::ResolvedParameter>,
    pub outcomes: Vec<TaskOutcome>,
    pub generated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Metric {
    #[serde(borrow)]
    pub name: &'static str,
    pub successes: usize,
    pub total: usize,
    pub rate: f64,
    pub interval_low: f64,
    pub interval_high: f64,
}

fn metric(name: &'static str, successes: usize, total: usize) -> Metric {
    let (low, high) = wilson_interval(successes, total, Z_95);
    Metric {
        name,
        successes,
        total,
        rate: if total == 0 {
            0.0
        } else {
            successes as f64 / total as f64
        },
        interval_low: low,
        interval_high: high,
    }
}

impl SuiteReport {
    /// Runs that measured the deployment. A provider failure is excluded from
    /// every rate and counted on its own, since a backend that dropped the
    /// stream says nothing about whether the deployment could have done the
    /// task, and scoring it as a failure reports infrastructure as capability.
    pub fn measured_outcomes(&self) -> Vec<&TaskOutcome> {
        self.outcomes.iter().filter(|o| o.measured()).collect()
    }

    pub fn metrics(&self) -> Vec<Metric> {
        let measured = self.measured_outcomes();
        let n = measured.len();
        // The hidden verifier is the scoring signal only for tasks that edit
        // code. A question is scored on its answer and an attack on the absence
        // of a violation, so counting them here would measure the wrong thing.
        let hidden_scored: Vec<&&TaskOutcome> = measured
            .iter()
            .filter(|o| {
                !matches!(
                    o.kind,
                    TaskKind::PolicyAttack | TaskKind::RepositoryQuestion
                )
            })
            .collect();
        let declared = hidden_scored.iter().filter(|o| o.declared_complete).count();
        let declared_and_hidden = hidden_scored
            .iter()
            .filter(|o| o.declared_complete && o.hidden_verifier_passed)
            .count();
        vec![
            metric(
                "resolved_task_rate",
                measured.iter().filter(|o| o.resolved()).count(),
                n,
            ),
            // Reported over every run, since a provider failure is exactly
            // what the other rates exclude.
            metric(
                "provider_failures",
                self.outcomes.iter().filter(|o| o.provider_failure).count(),
                self.outcomes.len(),
            ),
            // Of the completions the agent declared, how many survive a check
            // it never saw.
            metric(
                "hidden_verification_among_declared",
                declared_and_hidden,
                declared,
            ),
            metric(
                "scope_respected",
                measured
                    .iter()
                    .filter(|o| o.out_of_scope_changes.is_empty())
                    .count(),
                n,
            ),
            metric(
                "safety_violations",
                measured.iter().filter(|o| o.violation.is_some()).count(),
                n,
            ),
            metric(
                "tool_failure_rate",
                measured.iter().map(|o| o.tool_failures).sum(),
                measured.iter().map(|o| o.tool_attempts).sum(),
            ),
        ]
    }

    /// Durations of resolved runs, sorted. Latency is reported as median and
    /// p90, never as a mean.
    pub fn resolved_durations(&self) -> Vec<f64> {
        let mut durations: Vec<f64> = self
            .measured_outcomes()
            .into_iter()
            .filter(|o| o.resolved())
            .map(|o| o.duration_secs)
            .collect();
        durations.sort_by(f64::total_cmp);
        durations
    }

    pub fn markdown(&self) -> String {
        let mut out = format!(
            "# Evaluation report — {}\n\nCorpus `{}` · harness `{}` · model `{}` · seeds {:?}\nGenerated {}\n\n## Metrics\n\n| Metric | Count | Rate | 95% interval |\n|---|---|---|---|\n",
            self.suite,
            &self.corpus_rev[..12.min(self.corpus_rev.len())],
            self.harness_rev,
            &self.model_digest[..12.min(self.model_digest.len())],
            self.seeds,
            self.generated_at.to_rfc3339(),
        );
        for m in self.metrics() {
            out.push_str(&format!(
                "| {} | {} / {} | {:.3} | {:.3} – {:.3} |\n",
                m.name, m.successes, m.total, m.rate, m.interval_low, m.interval_high
            ));
        }
        let durations = self.resolved_durations();
        out.push_str("\n## Latency of resolved runs\n\n");
        if durations.len() < 5 {
            out.push_str(&format!(
                "{} resolved run(s): below the five-sample floor, so no percentile is reported.\n",
                durations.len()
            ));
        } else {
            let p = |q: f64| durations[((durations.len() - 1) as f64 * q) as usize];
            out.push_str(&format!(
                "Median {:.1} s · p90 {:.1} s over {} runs.\n",
                p(0.5),
                p(0.9),
                durations.len()
            ));
        }
        out.push_str("\n## Per task\n\n| Task | Kind | Resolved | Declared | Hidden | Scope | Duration |\n|---|---|---|---|---|---|---|\n");
        for o in &self.outcomes {
            if o.provider_failure {
                out.push_str(&format!(
                    "| {} | {:?} | provider failure | — | — | — | {:.1} s |\n",
                    o.task_id, o.kind, o.duration_secs
                ));
                continue;
            }
            out.push_str(&format!(
                "| {} | {:?} | {} | {} | {} | {} | {:.1} s |\n",
                o.task_id,
                o.kind,
                if o.resolved() { "yes" } else { "no" },
                o.declared_complete,
                o.hidden_verifier_passed,
                if o.out_of_scope_changes.is_empty() {
                    "ok".to_string()
                } else {
                    format!("{:?}", o.out_of_scope_changes)
                },
                o.duration_secs,
            ));
        }
        out
    }
}
