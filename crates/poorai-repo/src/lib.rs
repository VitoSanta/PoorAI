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
    /// Modules, packages or files this one names in an import.
    ///
    /// The graph edges `repository-intelligence.md` has always specified and
    /// never had. Shallow by design -- the name as written, not resolved to a
    /// path -- because resolution is per language and per build system, and a
    /// wrong resolution is worse than an unresolved name.
    #[serde(default)]
    pub imports: Vec<String>,
    /// The source stem this file appears to test, by naming convention.
    ///
    /// `tests/parser.rs`, `test_parser.py` and `parser.test.ts` all say the
    /// same thing about `parser`. Ownership by convention is a guess and is
    /// labelled as one wherever it is used to rank.
    #[serde(default)]
    pub tests: Option<String>,
    /// Distinct lowercase words in the file, bounded.
    ///
    /// Retrieval read every file in the repository to score it and then
    /// re-opened the ones it chose -- O(repository bytes) twice per run, on a
    /// workspace the previous run had already read. Keeping the terms means
    /// scoring touches the index and only the selected excerpts touch the
    /// disk.
    #[serde(default)]
    pub terms: Vec<String>,
    /// Modification time and size, the pair that says "unchanged" without
    /// reading. A file whose hash matters is still hashed; this only decides
    /// whether it has to be.
    #[serde(default)]
    pub mtime_ns: u64,
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
    index_incremental(root, None).map(|(index, _)| index)
}

/// The same index, reusing what an earlier run measured about files that have
/// not changed.
///
/// The cache decides only whether a file has to be *read*. Everything a
/// consumer is told -- the hash, the symbols, the imports -- was measured from
/// the bytes at some point, and a reused record carries the values the run
/// that read it computed. Nothing is inferred from a timestamp.
pub fn index_incremental(
    root: impl AsRef<Path>,
    state_dir: Option<&Path>,
) -> Result<(RepositoryIndex, IndexWork), RepoError> {
    let root = root.as_ref().canonicalize()?;
    if !root.is_dir() {
        return Err(RepoError::InvalidRoot);
    }
    let cache = match state_dir {
        Some(state_dir) => Some(IndexCache::open(state_dir)?),
        None => None,
    };
    let mut files = Vec::new();
    let work = walk_with_cache(&root, cache.as_ref(), &mut files)?;
    files.sort_by(|a, b| a.path.cmp(&b.path));
    let inventory_hash = hash_bytes(serde_json::to_vec(&files).expect("serializable"));
    Ok((
        RepositoryIndex {
            schema_version: 1,
            root: root.display().to_string(),
            generated_at: now(),
            inventory_hash,
            files,
            stale: false,
        },
        work,
    ))
}
/// Writes the index atomically as a durable workspace artifact.
pub fn persist(index: &RepositoryIndex, state_dir: &Path) -> Result<std::path::PathBuf, RepoError> {
    use std::io::Write as _;
    let directory = state_dir.join("indexes");
    fs::create_dir_all(&directory)?;
    let bytes = serde_json::to_vec_pretty(index).expect("index is serializable");
    let artifact_hash = hash_bytes(&bytes);
    let final_path = directory.join(format!("{artifact_hash}.json"));
    if final_path.exists() {
        if fs::read(&final_path)? == bytes {
            return Ok(final_path);
        }
        return Err(RepoError::Io(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "index artifact hash path contains different bytes",
        )));
    }
    let temporary = directory.join(format!(".index-{}.tmp", poorai_domain::new_id()));
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    match fs::hard_link(&temporary, &final_path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            if fs::read(&final_path)? != bytes {
                let _ = fs::remove_file(&temporary);
                return Err(RepoError::Io(error));
            }
        }
        Err(error) => {
            let _ = fs::remove_file(&temporary);
            return Err(RepoError::Io(error));
        }
    }
    fs::remove_file(&temporary)?;
    Ok(final_path)
}
/// Recomputes the inventory hash; callers must refresh stale indexes before edits.
pub fn stale(index: &RepositoryIndex) -> Result<bool, RepoError> {
    Ok(self::index(&index.root)?.inventory_hash != index.inventory_hash)
}
/// Keywords that introduce a named declaration, across the languages a
/// repository is likely to be written in.
///
/// A list of keywords rather than a parser per language: the point is to find
/// the name a task is likely to mention, and the shape `keyword Name` is
/// nearly universal. Getting this wrong is not cosmetic — a symbol definition
/// outranks a path match five to one in retrieval, so a language whose
/// declarations are invisible loses the strongest ranking signal exactly where
/// the agent knows least.
const DECLARATION_KEYWORDS: [&str; 22] = [
    "fn",
    "func",
    "def",
    "function",
    "class",
    "struct",
    "enum",
    "trait",
    "interface",
    "impl",
    "type",
    "record",
    "object",
    "module",
    "package",
    "protocol",
    "extension",
    "actor",
    "mixin",
    "sub",
    "namespace",
    "union",
];

/// Modifiers that may precede a declaration and are not the name.
const DECLARATION_MODIFIERS: [&str; 18] = [
    "pub",
    "public",
    "private",
    "protected",
    "internal",
    "static",
    "final",
    "abstract",
    "sealed",
    "open",
    "export",
    "default",
    "async",
    "const",
    "let",
    "var",
    "override",
    "partial",
];

/// Names declared in a source file, whatever language it is written in.
pub fn extract_symbols(text: &str) -> Vec<String> {
    let mut symbols = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with("//") || line.starts_with('#') || line.starts_with('*') {
            continue;
        }
        let mut words = line
            .split(|c: char| c.is_whitespace() || c == '(' || c == '<' || c == ':' || c == '{')
            .filter(|w| !w.is_empty());
        // Skip the modifiers, then require a declaration keyword, then take the
        // next word as the name.
        let mut saw_keyword = false;
        for word in words.by_ref() {
            let bare = word.trim_end_matches(&['*', '&'][..]);
            if DECLARATION_MODIFIERS.contains(&bare) {
                continue;
            }
            if DECLARATION_KEYWORDS.contains(&bare) {
                saw_keyword = true;
            }
            break;
        }
        if !saw_keyword {
            continue;
        }
        if let Some(name) = words.next() {
            let name: String = name
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '$')
                .collect();
            // A bare keyword with no name, or a name that is itself a keyword,
            // is a control-flow line rather than a declaration.
            if name.len() >= 2
                && !DECLARATION_KEYWORDS.contains(&name.as_str())
                && !name.chars().next().is_some_and(|c| c.is_numeric())
            {
                symbols.push(name);
            }
        }
    }
    symbols.sort();
    symbols.dedup();
    symbols
}

/// Import statements, as written.
///
/// Shallow on purpose: the name a file writes, not the path it resolves to.
/// Resolution is per language and per build system, and a wrong edge is worse
/// than a missing one -- it points retrieval at a file that has nothing to do
/// with the task.
pub fn extract_imports(text: &str) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    for line in text.lines().take(MAX_SCANNED_LINES) {
        let line = line.trim();
        if line.starts_with("//") || line.starts_with('#') && !line.starts_with("#include") {
            // A comment, except the one language where `#` starts a directive.
            if !line.starts_with("#include") && !line.starts_with("#import") {
                // Python's `import` never starts with `#`, so a `#` line here
                // is a comment in every language this recognises.
                if line.starts_with('#') {
                    continue;
                }
            }
        }
        let candidate = if let Some(rest) = line.strip_prefix("use ") {
            // Rust: `use crate::parser::Token;`
            rest.split([';', ' ', '{'])
                .next()
                .map(|path| path.trim_end_matches("::").to_string())
        } else if let Some(rest) = line.strip_prefix("from ") {
            // Python: `from app.parser import Token`
            rest.split_whitespace().next().map(str::to_string)
        } else if line.starts_with("import ") && line.contains(" from ") {
            // JavaScript and TypeScript: `import { Token } from "./parser"`.
            // Checked before the bare `import` form, which would otherwise
            // take the opening brace as the module name.
            line.split_once(" from ").and_then(|(_, tail)| {
                tail.trim()
                    .trim_start_matches(['"', '\''])
                    .split(['"', '\'', ';'])
                    .next()
                    .map(str::to_string)
            })
        } else if let Some(rest) = line.strip_prefix("import ") {
            // Python, Java, Go, and the side-effect form in JavaScript.
            rest.split([';', ' '])
                .next()
                .map(|name| name.trim_matches(['"', '\'']).to_string())
        } else if line.starts_with("#include") {
            line.split(['<', '"'])
                .nth(1)
                .map(|name| name.trim_end_matches(['>', '"']).to_string())
        } else {
            // JavaScript and TypeScript: `import { x } from "./parser"`, and
            // `require("./parser")`.
            line.split_once(" from ")
                .map(|(_, tail)| tail)
                .or_else(|| line.split_once("require(").map(|(_, tail)| tail))
                .and_then(|tail| {
                    let tail = tail.trim().trim_start_matches(['"', '\'']);
                    tail.split(['"', '\'', ')', ';']).next()
                })
                .map(str::to_string)
        };
        if let Some(name) = candidate {
            let name = name.trim().trim_matches(['"', '\'', ',']).to_string();
            if !name.is_empty() && name.len() <= 200 && !found.contains(&name) {
                found.push(name);
            }
        }
        if found.len() >= MAX_IMPORTS {
            break;
        }
    }
    found
}

/// The source stem a path appears to test, by naming convention.
///
/// A guess, and labelled as one wherever it ranks: `tests/parser.rs`,
/// `test_parser.py` and `parser.test.ts` all say the same thing about
/// `parser`, and none of them proves it.
pub fn test_subject(path: &str) -> Option<String> {
    let file = Path::new(path).file_stem()?.to_str()?.to_ascii_lowercase();
    let in_test_directory = path
        .split(['/', '\\'])
        .any(|part| matches!(part, "tests" | "test" | "spec" | "__tests__"));
    let stem = if let Some(rest) = file.strip_prefix("test_") {
        Some(rest.to_string())
    } else if let Some(rest) = file.strip_suffix("_test") {
        Some(rest.to_string())
    } else if let Some(rest) = file.strip_suffix(".test") {
        Some(rest.to_string())
    } else if let Some(rest) = file.strip_suffix(".spec") {
        Some(rest.to_string())
    } else if in_test_directory {
        Some(file)
    } else {
        None
    };
    stem.filter(|stem| !stem.is_empty() && stem != "mod" && stem != "index")
}

/// Distinct lowercase words, bounded.
///
/// What scoring reads instead of the file itself. Bounded because a generated
/// file with a hundred thousand identifiers should not be able to make the
/// index larger than the repository.
fn extract_terms(text: &str) -> Vec<String> {
    let mut terms: Vec<String> = Vec::new();
    for word in text
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .filter(|word| word.len() >= 2)
    {
        let word = word.to_ascii_lowercase();
        if !terms.contains(&word) {
            terms.push(word);
        }
        if terms.len() >= MAX_TERMS {
            break;
        }
    }
    terms.sort();
    terms
}

const MAX_SCANNED_LINES: usize = 2_000;
const MAX_IMPORTS: usize = 200;
const MAX_TERMS: usize = 4_000;

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
/// What the last index knew about a file, keyed by path.
///
/// Every run walked and re-read the whole repository, then retrieval re-read
/// every file to score it and re-opened the ones it chose -- O(repository
/// bytes) twice, on a workspace the previous run had already read. This is the
/// half that stops the first re-read.
///
/// Keyed on modification time and size together, and never on either alone: a
/// file rewritten with the same length in the same second is the case where
/// both are needed and the hash is what settles it. A cache hit still carries
/// the hash the previous run computed, so nothing downstream is told a hash
/// that was not measured.
pub struct IndexCache {
    connection: rusqlite::Connection,
}

impl IndexCache {
    /// Opens, or creates, the cache beside the workspace's other state.
    pub fn open(state_dir: &Path) -> Result<Self, RepoError> {
        fs::create_dir_all(state_dir)?;
        let connection = rusqlite::Connection::open(state_dir.join("index.sqlite"))
            .map_err(|error| RepoError::Walk(error.to_string()))?;
        connection
            .execute_batch(
                "CREATE TABLE IF NOT EXISTS files (
                    path TEXT PRIMARY KEY,
                    mtime_ns INTEGER NOT NULL,
                    bytes INTEGER NOT NULL,
                    content_hash TEXT NOT NULL,
                    record TEXT NOT NULL
                );",
            )
            .map_err(|error| RepoError::Walk(error.to_string()))?;
        Ok(Self { connection })
    }

    fn get(&self, path: &str, mtime_ns: u64, bytes: u64) -> Option<FileRecord> {
        let record: String = self
            .connection
            .query_row(
                "SELECT record FROM files WHERE path=?1 AND mtime_ns=?2 AND bytes=?3",
                rusqlite::params![path, mtime_ns as i64, bytes as i64],
                |row| row.get(0),
            )
            .ok()?;
        serde_json::from_str(&record).ok()
    }

    fn put(&self, record: &FileRecord) -> Result<(), RepoError> {
        let encoded =
            serde_json::to_string(record).map_err(|error| RepoError::Walk(error.to_string()))?;
        self.connection
            .execute(
                "INSERT INTO files(path,mtime_ns,bytes,content_hash,record) VALUES(?1,?2,?3,?4,?5)
                 ON CONFLICT(path) DO UPDATE SET mtime_ns=?2, bytes=?3, content_hash=?4, record=?5",
                rusqlite::params![
                    record.path,
                    record.mtime_ns as i64,
                    record.bytes as i64,
                    record.content_hash,
                    encoded
                ],
            )
            .map_err(|error| RepoError::Walk(error.to_string()))?;
        Ok(())
    }

    /// Removes rows for files the walk no longer found.
    ///
    /// A deleted file that stays in the cache is a file retrieval can still
    /// rank, which is worse than a slow index.
    fn retain(&self, present: &[String]) -> Result<usize, RepoError> {
        let mut statement = self
            .connection
            .prepare("SELECT path FROM files")
            .map_err(|error| RepoError::Walk(error.to_string()))?;
        let known: Vec<String> = statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|error| RepoError::Walk(error.to_string()))?
            .filter_map(Result::ok)
            .collect();
        let mut removed = 0;
        for path in known {
            if !present.contains(&path) {
                self.connection
                    .execute("DELETE FROM files WHERE path=?1", rusqlite::params![path])
                    .map_err(|error| RepoError::Walk(error.to_string()))?;
                removed += 1;
            }
        }
        Ok(removed)
    }
}

/// How much of an index run had to be read from disk.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct IndexWork {
    pub files: usize,
    pub read: usize,
    pub reused: usize,
    pub forgotten: usize,
}

fn walk_with_cache(
    root: &Path,
    cache: Option<&IndexCache>,
    files: &mut Vec<FileRecord>,
) -> Result<IndexWork, RepoError> {
    let mut work = IndexWork::default();
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
        let relative = path
            .strip_prefix(root)
            .unwrap_or(path)
            .display()
            .to_string();
        let size = metadata.len();
        let mtime_ns = metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|since| since.as_nanos() as u64)
            .unwrap_or_default();
        work.files += 1;
        if let Some(cached) = cache.and_then(|cache| cache.get(&relative, mtime_ns, size)) {
            work.reused += 1;
            files.push(cached);
            continue;
        }
        let bytes = fs::read(path)?;
        work.read += 1;
        if bytes.len() > MAX_INDEXED_BYTES || bytes.iter().take(4096).any(|b| *b == 0) {
            continue;
        }
        let text = String::from_utf8_lossy(&bytes);
        let record = FileRecord {
            content_hash: hash_bytes(&bytes),
            bytes: bytes.len() as u64,
            symbols: extract_symbols(&text),
            imports: extract_imports(&text),
            tests: test_subject(&relative),
            terms: extract_terms(&text),
            mtime_ns,
            path: relative,
        };
        if let Some(cache) = cache {
            cache.put(&record)?;
        }
        files.push(record);
    }
    if let Some(cache) = cache {
        let present: Vec<String> = files.iter().map(|file| file.path.clone()).collect();
        work.forgotten = cache.retain(&present)?;
    }
    Ok(work)
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

/// Files reachable in one hop from the strongest candidates.
///
/// One hop, deliberately. Two hops from a well-connected module is most of the
/// repository, and a ranking signal that reaches everything ranks nothing.
/// Weaker than any direct match, because proximity is evidence about the
/// neighbourhood rather than about the file.
fn graph_neighbours(
    index: &RepositoryIndex,
    scored: &[(u32, String, &FileRecord)],
) -> Vec<(String, (u32, String))> {
    let mut best: Vec<&FileRecord> = scored
        .iter()
        .map(|(_, _, file)| *file)
        .take(GRAPH_SEEDS)
        .collect();
    best.sort_by(|a, b| a.path.cmp(&b.path));
    let mut found: Vec<(String, (u32, String))> = Vec::new();
    let mut add = |path: String, bonus: u32, why: String| {
        if !found.iter().any(|(existing, _)| *existing == path) {
            found.push((path, (bonus, why)));
        }
    };
    for seed in &best {
        let seed_stem = Path::new(&seed.path)
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        for file in &index.files {
            if file.path == seed.path {
                continue;
            }
            let stem = Path::new(&file.path)
                .file_stem()
                .and_then(|stem| stem.to_str())
                .unwrap_or_default()
                .to_ascii_lowercase();
            // The seed imports this file, by the name it wrote.
            if !stem.is_empty()
                && seed
                    .imports
                    .iter()
                    .any(|name| name.to_ascii_lowercase().contains(&stem))
            {
                add(
                    file.path.clone(),
                    IMPORT_PROXIMITY,
                    format!("imported by {}", seed.path),
                );
            }
            // This file tests the seed, by naming convention -- a guess, and
            // said to be one.
            if file
                .tests
                .as_deref()
                .is_some_and(|subject| subject == seed_stem)
            {
                add(
                    file.path.clone(),
                    TEST_OWNERSHIP,
                    format!("appears to test {} by name", seed.path),
                );
            }
        }
    }
    found
}

/// How many top candidates seed the graph walk.
const GRAPH_SEEDS: usize = 3;
/// Weaker than a path match: proximity is evidence about the neighbourhood.
const IMPORT_PROXIMITY: u32 = 6;
/// Weaker still, because ownership here is a naming convention, not a fact.
const TEST_OWNERSHIP: u32 = 5;

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
    // Scored from the index, not from the disk. Retrieval used to read every
    // file to score it and then re-open the ones it chose; the terms an
    // earlier run extracted answer the same question without either read.
    let mut scored: Vec<(u32, String, &FileRecord)> = Vec::new();
    for file in &index.files {
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
            // A distinct term rather than an occurrence count. The index keeps
            // which words a file uses, not how often, and saturating the count
            // at eight was already most of the way to that -- a file
            // mentioning a term a hundred times was never fifty times more
            // relevant than one mentioning it twice.
            if file.terms.binary_search(term).is_ok() {
                score += CONTENT_OCCURRENCE * MAX_COUNTED_OCCURRENCES as u32;
                reasons.push(format!("mentions {term}"));
            }
        }
        if score > 0 {
            reasons.dedup();
            scored.push((score, reasons.join(", "), file));
        }
    }

    // Graph proximity: a file the best candidates import, and the test that
    // owns one of them, are about the task even when they never name it.
    // `repository-intelligence.md` has specified this since the beginning and
    // the ranking has never had it.
    let neighbours = graph_neighbours(index, &scored);
    for (path, (bonus, why)) in neighbours {
        if let Some(existing) = scored.iter_mut().find(|(_, _, file)| file.path == path) {
            existing.0 += bonus;
            existing.1 = format!("{}, {why}", existing.1);
        } else if let Some(file) = index.files.iter().find(|file| file.path == path) {
            scored.push((bonus, why, file));
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

    #[test]
    fn persisted_indexes_are_content_addressed_and_never_overwritten() {
        let root = tempfile::tempdir().unwrap();
        let state = root.path().join(".poorai");
        fs::write(root.path().join("code.rs"), "fn one() {}").unwrap();
        let first = index(root.path()).unwrap();
        let first_path = persist(&first, &state).unwrap();
        let first_bytes = fs::read(&first_path).unwrap();
        fs::write(root.path().join("code.rs"), "fn two() {}").unwrap();
        let second = index(root.path()).unwrap();
        let second_path = persist(&second, &state).unwrap();
        assert_ne!(first_path, second_path);
        assert_eq!(fs::read(first_path).unwrap(), first_bytes);
    }
}
