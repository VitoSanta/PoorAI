//! Typed, bounded workspace tools and policy enforcement.
use poorai_domain::hash_bytes;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::{
    path::{Component, Path, PathBuf},
    time::Duration,
};
use tokio::{process::Command, time::timeout};

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
    RunCommand {
        executable: String,
        args: Vec<String>,
    },
}
impl ActionProposal {
    pub fn validate(&self) -> Result<(), ToolError> {
        match self {
            Self::Complete { rationale } if rationale.is_empty() => {
                Err(ToolError::Denied("completion rationale is required".into()))
            }
            Self::ReadFile { path } | Self::ApplyReplace { path, .. } if path.is_empty() => {
                Err(ToolError::Denied("action path is empty".into()))
            }
            Self::Search { query, max_matches } if query.is_empty() || *max_matches == 0 => Err(
                ToolError::Denied("search query and limit are required".into()),
            ),
            Self::ListTree { max_entries } if *max_entries == 0 => {
                Err(ToolError::Denied("tree limit is required".into()))
            }
            Self::RunCommand { executable, .. } if executable.is_empty() => {
                Err(ToolError::Denied("executable is required".into()))
            }
            _ => Ok(()),
        }
    }
}
#[derive(Debug, Clone)]
pub struct ToolPolicy {
    pub root: PathBuf,
    pub allow_commands: Vec<String>,
    pub output_limit: usize,
    pub timeout: Duration,
    pub network_enabled: bool,
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
            network_enabled: false,
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
            let parent = result
                .parent()
                .ok_or_else(|| ToolError::Denied("target has no parent".into()))?
                .canonicalize()
                .map_err(|_| ToolError::Denied("target parent is unavailable".into()))?;
            parent.join(
                result
                    .file_name()
                    .ok_or_else(|| ToolError::Denied("target has no file name".into()))?,
            )
        };
        if !checked.starts_with(&root) {
            return Err(ToolError::Denied("path escapes workspace root".into()));
        }
        Ok(checked)
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
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileReadResult {
    pub path: String,
    pub content: String,
    pub truncated: bool,
    pub artifact_hash: String,
    pub redacted: bool,
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
    let path = policy.resolve(relative)?;
    let existing = std::fs::read(&path)?;
    if existing.iter().take(4096).any(|b| *b == 0) {
        return Err(ToolError::Denied("binary edits are denied".into()));
    }
    let previous_hash = hash_bytes(&existing);
    if previous_hash != expected_hash {
        return Err(ToolError::Denied(
            "stale file hash; reread before editing".into(),
        ));
    }
    if replacement.len() > policy.output_limit {
        return Err(ToolError::Denied(
            "replacement exceeds policy size limit".into(),
        ));
    }
    std::fs::write(&path, replacement.as_bytes())?;
    Ok(ApplyResult {
        path: relative.display().to_string(),
        previous_hash,
        new_hash: hash_bytes(replacement.as_bytes()),
    })
}

/// Reads a root-relative text file with a policy-derived byte bound.
pub fn read_file(policy: &ToolPolicy, relative: &Path) -> Result<FileReadResult, ToolError> {
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
    let truncated = bytes.len() > policy.output_limit;
    let content =
        String::from_utf8_lossy(&bytes[..bytes.len().min(policy.output_limit)]).to_string();
    let (content, redacted) = policy.redact(&content);
    Ok(FileReadResult {
        path: relative.display().to_string(),
        artifact_hash: hash_bytes(&bytes),
        content,
        truncated,
        redacted,
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
    if !policy.network_enabled
        && args
            .iter()
            .any(|a| a.contains("http://") || a.contains("https://"))
    {
        return Err(ToolError::Denied("network is disabled by policy".into()));
    }
    let started = std::time::Instant::now();
    let output = timeout(
        policy.timeout,
        Command::new(executable)
            .args(args)
            .current_dir(&policy.root)
            .env_clear()
            // PATH is explicitly allowlisted solely for executable resolution; it is never logged.
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
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
            network_enabled: false,
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
            network_enabled: false,
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
            network_enabled: false,
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
            network_enabled: false,
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
            network_enabled: false,
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
            network_enabled: false,
        };
        assert!(list_tree(&policy, 10).unwrap().is_empty());
        assert!(search(&policy, "needle", 10).unwrap().is_empty());
    }
    #[test]
    fn development_profile_still_denies_network() {
        let policy = PolicyProfile::Development.build(PathBuf::from("/tmp"));
        assert!(!policy.network_enabled);
        assert!(policy.allow_commands.contains(&"cargo".into()));
    }
}
