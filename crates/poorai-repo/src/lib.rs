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

/// A ranked passage of the repository, with everything needed to audit why it
/// was chosen and what it cost.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Excerpt {
    pub path: String,
    /// One-based and inclusive.
    pub first_line: usize,
    pub last_line: usize,
    pub content: String,
    /// Hash of the whole file, so an edit guarded by it stays sound.
    pub content_hash: String,
    /// Which signals selected this passage, in the words of the signals.
    pub rationale: String,
    /// An estimate. Token counts are provider-specific and only exact when a
    /// backend reports them, so this is labelled rather than presented as one.
    pub estimated_tokens: usize,
}

/// Words too common to discriminate between files.
const STOP_WORDS: [&str; 24] = [
    "the", "and", "for", "that", "with", "this", "from", "into", "must", "should", "when", "where",
    "which", "what", "does", "not", "you", "are", "was", "has", "have", "its", "it's", "make",
];

/// Weight per signal. Named constants rather than inline numbers so a ranking
/// decision can be argued with instead of reverse-engineered.
const SYMBOL_EXACT: u32 = 40;
const SYMBOL_PARTIAL: u32 = 12;
const PATH_MATCH: u32 = 8;
const CONTENT_OCCURRENCE: u32 = 2;
/// Occurrences beyond this add nothing: a file that mentions a term a hundred
/// times is not fifty times more relevant than one that mentions it twice.
const MAX_COUNTED_OCCURRENCES: usize = 8;
/// Lines of context kept either side of the densest match.
const EXCERPT_CONTEXT_LINES: usize = 12;
/// Characters per token. A rough, documented estimate, not a count.
const CHARS_PER_TOKEN: usize = 4;

fn terms_of(query: &str) -> Vec<String> {
    let mut terms: Vec<String> = query
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .map(|w| w.to_lowercase())
        .filter(|w| w.len() >= 3 && !STOP_WORDS.contains(&w.as_str()))
        .collect();
    terms.sort();
    terms.dedup();
    terms
}

/// Ranks repository passages against a task statement.
///
/// This is lexical: it matches symbol names, path components and literal
/// occurrences. It does not understand the code, and a passage it ranks first
/// is the one that mentions the task's words most, which is not the same as the
/// one that matters most. The rationale on every excerpt says which signals
/// fired, so a wrong retrieval is diagnosable rather than mysterious.
pub fn retrieve(
    root: &Path,
    index: &RepositoryIndex,
    query: &str,
    max_excerpts: usize,
    token_budget: usize,
) -> Result<Vec<Excerpt>, RepoError> {
    let terms = terms_of(query);
    if terms.is_empty() || max_excerpts == 0 {
        return Ok(Vec::new());
    }
    let mut scored: Vec<(u32, String, &FileRecord)> = Vec::new();
    for file in &index.files {
        let text = fs::read_to_string(root.join(&file.path)).unwrap_or_default();
        let lowered = text.to_lowercase();
        let lowered_path = file.path.to_lowercase();
        let mut score = 0u32;
        let mut reasons = Vec::new();
        for term in &terms {
            if file.symbols.iter().any(|s| s.to_lowercase() == *term) {
                score += SYMBOL_EXACT;
                reasons.push(format!("defines {term}"));
            } else if file.symbols.iter().any(|s| s.to_lowercase().contains(term)) {
                score += SYMBOL_PARTIAL;
                reasons.push(format!("symbol mentions {term}"));
            }
            if lowered_path.contains(term) {
                score += PATH_MATCH;
                reasons.push(format!("path contains {term}"));
            }
            let occurrences = lowered.matches(term.as_str()).count();
            if occurrences > 0 {
                score += CONTENT_OCCURRENCE * occurrences.min(MAX_COUNTED_OCCURRENCES) as u32;
                reasons.push(format!("mentions {term} {occurrences}x"));
            }
        }
        if score > 0 {
            reasons.dedup();
            scored.push((score, reasons.join(", "), file));
        }
    }
    scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.2.path.cmp(&b.2.path)));

    let mut excerpts = Vec::new();
    let mut spent = 0usize;
    for (score, rationale, file) in scored.into_iter().take(max_excerpts) {
        let text = match fs::read_to_string(root.join(&file.path)) {
            Ok(text) => text,
            Err(_) => continue,
        };
        let lines: Vec<&str> = text.lines().collect();
        if lines.is_empty() {
            continue;
        }
        // The densest line wins the window, so an excerpt is centred on the
        // evidence rather than on the top of the file.
        let best = lines
            .iter()
            .enumerate()
            .max_by_key(|(_, line)| {
                let lowered = line.to_lowercase();
                terms
                    .iter()
                    .filter(|t| lowered.contains(t.as_str()))
                    .count()
            })
            .map(|(i, _)| i)
            .unwrap_or(0);
        let first = best.saturating_sub(EXCERPT_CONTEXT_LINES);
        let last = (best + EXCERPT_CONTEXT_LINES).min(lines.len() - 1);
        let content = lines[first..=last].join(
            "
",
        );
        let estimated_tokens = content.len().div_ceil(CHARS_PER_TOKEN);
        if spent + estimated_tokens > token_budget {
            continue;
        }
        spent += estimated_tokens;
        excerpts.push(Excerpt {
            path: file.path.clone(),
            first_line: first + 1,
            last_line: last + 1,
            content,
            content_hash: file.content_hash.clone(),
            rationale: format!("score {score}: {rationale}"),
            estimated_tokens,
        });
    }
    Ok(excerpts)
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
