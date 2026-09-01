//! Gitignore semantics the index must honour.
//!
//! Every rule a walk misunderstands is a file in the index that the repository
//! asked to keep out — which for `.env`, key material and build output is a
//! secret leak, not a cosmetic difference.

use std::fs;
use std::path::Path;

fn indexed(root: &Path) -> Vec<String> {
    let mut paths: Vec<String> = poorai_repo::index(root)
        .unwrap()
        .files
        .into_iter()
        .map(|file| file.path)
        .collect();
    paths.sort();
    paths
}

fn write(root: &Path, relative: &str, content: &str) {
    let path = root.join(relative);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, content).unwrap();
}

#[test]
fn negation_reinstates_a_previously_ignored_file() {
    let root = tempfile::tempdir().unwrap();
    write(root.path(), ".gitignore", "*.log\n!keep.log\n");
    write(root.path(), "drop.log", "x");
    write(root.path(), "keep.log", "x");
    let files = indexed(root.path());
    assert!(files.contains(&"keep.log".to_string()));
    assert!(!files.contains(&"drop.log".to_string()));
}

#[test]
fn leading_slash_anchors_a_pattern_to_the_root() {
    let root = tempfile::tempdir().unwrap();
    write(root.path(), ".gitignore", "/config.toml\n");
    write(root.path(), "config.toml", "x");
    write(root.path(), "nested/config.toml", "x");
    let files = indexed(root.path());
    assert!(!files.contains(&"config.toml".to_string()));
    // An anchored pattern must not reach into subdirectories.
    assert!(
        files
            .iter()
            .any(|path| path.ends_with("nested/config.toml"))
    );
}

#[test]
fn double_star_matches_across_directory_levels() {
    let root = tempfile::tempdir().unwrap();
    write(root.path(), ".gitignore", "**/secrets/**\n");
    write(root.path(), "a/secrets/key.pem", "x");
    write(root.path(), "a/b/secrets/key.pem", "x");
    write(root.path(), "a/visible.txt", "x");
    let files = indexed(root.path());
    assert!(!files.iter().any(|path| path.contains("secrets")));
    assert!(files.iter().any(|path| path.ends_with("visible.txt")));
}

#[test]
fn trailing_slash_ignores_a_directory_but_not_a_like_named_file() {
    let root = tempfile::tempdir().unwrap();
    write(root.path(), ".gitignore", "build/\n");
    write(root.path(), "build/output.o", "x");
    write(root.path(), "build.rs", "fn main() {}");
    let files = indexed(root.path());
    assert!(!files.iter().any(|path| path.starts_with("build/")));
    assert!(files.contains(&"build.rs".to_string()));
}

#[test]
fn a_nested_gitignore_applies_to_its_own_subtree_only() {
    let root = tempfile::tempdir().unwrap();
    write(root.path(), "vendor/.gitignore", "*.rs\n");
    write(root.path(), "vendor/generated.rs", "x");
    write(root.path(), "src/main.rs", "fn main() {}");
    let files = indexed(root.path());
    assert!(!files.iter().any(|path| path.ends_with("generated.rs")));
    assert!(files.iter().any(|path| path.ends_with("main.rs")));
}

#[test]
fn character_classes_are_honoured() {
    let root = tempfile::tempdir().unwrap();
    write(root.path(), ".gitignore", "temp[0-9].txt\n");
    write(root.path(), "temp1.txt", "x");
    write(root.path(), "tempa.txt", "x");
    let files = indexed(root.path());
    assert!(!files.contains(&"temp1.txt".to_string()));
    assert!(files.contains(&"tempa.txt".to_string()));
}

#[test]
fn a_dotfile_is_indexed_unless_the_repository_ignores_it() {
    let root = tempfile::tempdir().unwrap();
    write(root.path(), ".gitignore", ".env\n");
    write(root.path(), ".env", "API_KEY=live");
    write(root.path(), ".rustfmt.toml", "edition = \"2024\"");
    let files = indexed(root.path());
    // The canonical secret-leak case.
    assert!(!files.contains(&".env".to_string()));
    // Hidden files are not ignored merely for being hidden.
    assert!(files.contains(&".rustfmt.toml".to_string()));
}

#[test]
fn policy_exclusions_apply_even_when_the_repository_does_not_ignore_them() {
    let root = tempfile::tempdir().unwrap();
    // No .gitignore at all: these are poorAI policy, not repository preference.
    write(root.path(), ".git/config", "[core]");
    write(root.path(), ".poorai/state.json", "{}");
    write(root.path(), "target/debug/build.log", "x");
    write(root.path(), "node_modules/pkg/index.js", "x");
    write(root.path(), "src/main.rs", "fn main() {}");
    assert_eq!(indexed(root.path()), vec!["src/main.rs".to_string()]);
}

#[cfg(unix)]
#[test]
fn a_symlink_out_of_the_workspace_is_not_indexed_as_workspace_content() {
    use std::os::unix::fs::symlink;
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    fs::write(outside.path().join("credentials"), "AKIAIOSFODNN7EXAMPLE").unwrap();
    write(root.path(), "src/main.rs", "fn main() {}");
    symlink(outside.path().join("credentials"), root.path().join("link")).unwrap();
    let files = indexed(root.path());
    assert!(!files.contains(&"link".to_string()));
    assert_eq!(files, vec!["src/main.rs".to_string()]);
}

#[test]
fn a_stale_index_is_detectable_after_an_ignored_file_appears() {
    let root = tempfile::tempdir().unwrap();
    write(root.path(), ".gitignore", "*.log\n");
    write(root.path(), "src/main.rs", "fn main() {}");
    let artifact = poorai_repo::index(root.path()).unwrap();
    assert!(!poorai_repo::stale(&artifact).unwrap());
    // An ignored file changes nothing the index claims, so it is not stale.
    write(root.path(), "noise.log", "x");
    assert!(!poorai_repo::stale(&artifact).unwrap());
    // A tracked file does.
    write(root.path(), "src/main.rs", "fn main() { changed() }");
    assert!(poorai_repo::stale(&artifact).unwrap());
}
