//! Persistable repository inventory; repository contents are untrusted input.
use poorai_domain::{hash_bytes, now};
use serde::{Deserialize, Serialize};
use std::{fs, path::Path};
#[derive(Debug, thiserror::Error)]
pub enum RepoError {
    #[error("repository I/O failure: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid repository root")]
    InvalidRoot,
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
    let ignores = load_ignores(&root)?;
    visit(&root, &root, &ignores, &mut files)?;
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
fn load_ignores(root: &Path) -> Result<Vec<String>, RepoError> {
    let path = root.join(".gitignore");
    if !path.exists() {
        return Ok(vec![]);
    }
    Ok(String::from_utf8_lossy(&fs::read(path)?)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#') && !line.starts_with('!'))
        .map(|line| {
            line.trim_start_matches('/')
                .trim_end_matches('/')
                .to_string()
        })
        .collect())
}
fn ignored(relative: &Path, patterns: &[String]) -> bool {
    let text = relative.to_string_lossy();
    patterns.iter().any(|pattern| {
        text == pattern.as_str()
            || text.starts_with(&format!("{pattern}/"))
            || (pattern.starts_with('*') && text.ends_with(&pattern[1..]))
    })
}
fn visit(
    root: &Path,
    path: &Path,
    ignores: &[String],
    files: &mut Vec<FileRecord>,
) -> Result<(), RepoError> {
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let p = entry.path();
        let name = entry.file_name();
        if [".git", "target", "node_modules", ".poorai"]
            .iter()
            .any(|x| name == *x)
        {
            continue;
        }
        if ignored(p.strip_prefix(root).unwrap_or(&p), ignores) {
            continue;
        }
        if p.is_dir() {
            visit(root, &p, ignores, files)?;
        } else if p.is_file() {
            let bytes = fs::read(&p)?;
            if bytes.len() > 1_000_000 || bytes.iter().take(4096).any(|b| *b == 0) {
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
                path: p.strip_prefix(root).unwrap_or(&p).display().to_string(),
                content_hash: hash_bytes(&bytes),
                bytes: bytes.len() as u64,
                symbols,
            });
        }
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
