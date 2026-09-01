//! Persistable repository inventory; repository contents are untrusted input.
use ignore::WalkBuilder;
use poorai_domain::{hash_bytes, now};
use serde::{Deserialize, Serialize};
use std::{fs, path::Path};
#[derive(Debug, thiserror::Error)]
pub enum RepoError {
    #[error("repository I/O failure: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid repository root")]
    InvalidRoot,
    #[error("repository walk failure: {0}")]
    Walk(String),
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileRecord {
    pub path: String,
    pub content_hash: String,
    pub bytes: u64,
    pub symbols: Vec<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepositoryIndex {
    pub schema_version: u32,
    pub root: String,
    pub generated_at: chrono::DateTime<chrono::Utc>,
    pub inventory_hash: String,
    pub files: Vec<FileRecord>,
    pub stale: bool,
}
pub fn index(root: impl AsRef<Path>) -> Result<RepositoryIndex, RepoError> {
    let root = root.as_ref().canonicalize()?;
    if !root.is_dir() {
        return Err(RepoError::InvalidRoot);
    }
    let mut files = Vec::new();
    walk(&root, &mut files)?;
    files.sort_by(|a, b| a.path.cmp(&b.path));
    let inventory_hash = hash_bytes(serde_json::to_vec(&files).expect("serializable"));
    Ok(RepositoryIndex {
        schema_version: 1,
        root: root.display().to_string(),
        generated_at: now(),
        inventory_hash,
        files,
        stale: false,
    })
}
/// Writes the index atomically as a durable workspace artifact.
pub fn persist(index: &RepositoryIndex, state_dir: &Path) -> Result<std::path::PathBuf, RepoError> {
    fs::create_dir_all(state_dir)?;
    let final_path = state_dir.join("index.json");
    let temporary = state_dir.join("index.json.tmp");
    fs::write(
        &temporary,
        serde_json::to_vec_pretty(index).expect("index is serializable"),
    )?;
    fs::rename(&temporary, &final_path)?;
    Ok(final_path)
}
/// Recomputes the inventory hash; callers must refresh stale indexes before edits.
pub fn stale(index: &RepositoryIndex) -> Result<bool, RepoError> {
    Ok(self::index(&index.root)?.inventory_hash != index.inventory_hash)
}
/// poorAI exclusions applied on top of VCS ignore rules. These are policy, not
/// convenience: state and VCS internals must never enter the index.
const POLICY_EXCLUSIONS: [&str; 4] = [".git", "target", "node_modules", ".poorai"];
/// Files above this size are inventory entries only; the index is not a mirror.
const MAX_INDEXED_BYTES: usize = 1_000_000;

/// Walks the repository under full gitignore semantics.
///
/// Ignore rules are delegated to the same implementation ripgrep uses rather
/// than approximated here: negation, `**`, anchoring, directory-only patterns
/// and nested `.gitignore` files are all load-bearing, and a pattern this walk
/// misunderstands is a secret in the index.
fn walk(root: &Path, files: &mut Vec<FileRecord>) -> Result<(), RepoError> {
    let walker = WalkBuilder::new(root)
        .hidden(false)
        .git_ignore(true)
        // Ignore rules apply whether or not the root is a git checkout: a
        // workspace is untrusted input either way.
        .require_git(false)
        .git_global(false)
        .git_exclude(true)
        .parents(false)
        .follow_links(false)
        .filter_entry(|entry| {
            !POLICY_EXCLUSIONS
                .iter()
                .any(|blocked| entry.file_name() == *blocked)
        })
        .build();
    for entry in walker {
        let entry = entry.map_err(|error| RepoError::Walk(error.to_string()))?;
        let path = entry.path();
        // `symlink_metadata` keeps a link to a file outside the root from being
        // indexed as if it lived inside it.
        let metadata = fs::symlink_metadata(path)?;
        if !metadata.is_file() {
            continue;
        }
        let bytes = fs::read(path)?;
        if bytes.len() > MAX_INDEXED_BYTES || bytes.iter().take(4096).any(|b| *b == 0) {
            continue;
        }
        let text = String::from_utf8_lossy(&bytes);
        let symbols = text
            .lines()
            .filter_map(|l| {
                l.trim()
                    .strip_prefix("fn ")
                    .or_else(|| l.trim().strip_prefix("pub fn "))
            })
            .filter_map(|r| r.split('(').next())
            .map(str::to_string)
            .collect();
        files.push(FileRecord {
            path: path
                .strip_prefix(root)
                .unwrap_or(path)
                .display()
                .to_string(),
            content_hash: hash_bytes(&bytes),
            bytes: bytes.len() as u64,
            symbols,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ignores_gitignore_paths() {
        let root = tempfile::tempdir().unwrap();
        fs::write(
            root.path().join(".gitignore"),
            "secret.txt\ngenerated/\n*.log\n",
        )
        .unwrap();
        fs::write(root.path().join("secret.txt"), "no").unwrap();
        fs::write(root.path().join("visible.txt"), "yes").unwrap();
        fs::create_dir(root.path().join("generated")).unwrap();
        fs::write(root.path().join("generated/out.rs"), "no").unwrap();
        fs::write(root.path().join("debug.log"), "no").unwrap();
        let result = index(root.path()).unwrap();
        assert_eq!(result.files.len(), 2);
        assert!(result.files.iter().any(|file| file.path == "visible.txt"));
    }
    #[test]
    fn index_becomes_stale_after_content_change() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("code.rs");
        fs::write(&file, "fn one() {}").unwrap();
        let artifact = index(root.path()).unwrap();
        assert!(!stale(&artifact).unwrap());
        fs::write(&file, "fn two() {}").unwrap();
        assert!(stale(&artifact).unwrap());
    }
}
