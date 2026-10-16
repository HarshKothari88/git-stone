//! Where a stone sits in its chain: its base, the latest tree, and tree-to-tree change lists.
//!
//! Phase 3 only knows "HEAD is the anchor or it is not". The full classification and
//! auto-reconcile (A9) arrive in Phase 5.

use crate::git::{Git, GitError};
use crate::repo::Repo;
use crate::state::State;

/// The tree of the anchor commit, or the empty tree on an unborn branch.
pub fn anchor_tree(repo: &Repo, state: &State) -> Result<String, GitError> {
    commit_tree(repo, state.anchor.as_deref())
}

/// The tree of HEAD, or the empty tree on an unborn branch.
pub fn head_tree(repo: &Repo) -> Result<String, GitError> {
    commit_tree(repo, repo.head.as_deref())
}

fn commit_tree(repo: &Repo, commit: Option<&str>) -> Result<String, GitError> {
    match commit {
        None => Ok(repo.empty_tree.clone()),
        Some(oid) => {
            let out = repo.git().run(["rev-parse", &format!("{oid}^{{tree}}")])?;
            Ok(String::from_utf8_lossy(&out).trim().to_string())
        }
    }
}

/// The tree a stone's diff starts from: the previous stone's tree, else the anchor's (I3).
pub fn base_of(repo: &Repo, state: &State, index: usize) -> Result<String, GitError> {
    match index.checked_sub(1) {
        Some(prev) => Ok(state.stones[prev].tree.clone()),
        None => anchor_tree(repo, state),
    }
}

/// The tree the next stone would start from: the last stone's tree, else the anchor's.
pub fn latest_tree(repo: &Repo, state: &State) -> Result<String, GitError> {
    match state.stones.last() {
        Some(last) => Ok(last.tree.clone()),
        None => anchor_tree(repo, state),
    }
}

/// What "current changes" are measured against. With no stones that is HEAD itself, so a state
/// left over from an older commit does not matter.
pub fn current_base(repo: &Repo, state: Option<&State>) -> Result<String, GitError> {
    match state.and_then(|s| s.stones.last()) {
        Some(last) => Ok(last.tree.clone()),
        None => head_tree(repo),
    }
}

/// True when stones exist and the branch is no longer at the commit they were made on.
pub fn moved(repo: &Repo, state: &State) -> bool {
    !state.stones.is_empty() && repo.head != state.anchor
}

/// Phase 5 will recognise commits that already match the first stones. Until then nothing is safe to reconcile.
pub fn try_reconcile(_repo: &Repo, _state: &State) -> Result<bool, GitError> {
    Ok(false)
}

#[derive(Debug, PartialEq, Eq)]
pub struct FileStat {
    /// `None` for a binary file.
    pub added: Option<u64>,
    pub deleted: Option<u64>,
    /// A rename shows as `old -> new`.
    pub path: String,
}

/// `git diff-tree -r -z --numstat -M a b`
pub fn numstat(git: &Git, a: &str, b: &str) -> Result<Vec<FileStat>, GitError> {
    let out = git.run(["diff-tree", "-r", "-z", "--numstat", "-M", a, b])?;
    Ok(parse_numstat(&out))
}

fn parse_numstat(out: &[u8]) -> Vec<FileStat> {
    // Paths are never empty, so empty tokens (the one after the last NUL) can be skipped.
    let mut tokens = out.split(|b| *b == 0).filter(|t| !t.is_empty());
    let mut stats = Vec::new();
    while let Some(token) = tokens.next() {
        let text = String::from_utf8_lossy(token);
        let mut parts = text.splitn(3, '\t');
        let (Some(added), Some(deleted), Some(path)) = (parts.next(), parts.next(), parts.next())
        else {
            continue;
        };
        // A rename has an empty path here and the two real paths in the next two tokens.
        let path = if path.is_empty() {
            let old = tokens
                .next()
                .map(String::from_utf8_lossy)
                .unwrap_or_default();
            let new = tokens
                .next()
                .map(String::from_utf8_lossy)
                .unwrap_or_default();
            format!("{old} -> {new}")
        } else {
            path.to_string()
        };
        stats.push(FileStat {
            added: added.parse().ok(),
            deleted: deleted.parse().ok(),
            path,
        });
    }
    stats
}

/// `3 files changed, +40 -2`
pub fn summary(stats: &[FileStat]) -> String {
    if stats.is_empty() {
        return "no changes".to_string();
    }
    let added: u64 = stats.iter().filter_map(|s| s.added).sum();
    let deleted: u64 = stats.iter().filter_map(|s| s.deleted).sum();
    let files = stats.len();
    let noun = if files == 1 { "file" } else { "files" };
    format!("{files} {noun} changed, +{added} -{deleted}")
}

#[derive(Debug, PartialEq, Eq)]
pub struct Change {
    /// `A`, `M`, `D`, `R`, `T`, ...
    pub code: char,
    /// A rename shows as `old -> new`.
    pub path: String,
}

/// `git diff-tree -r -z --name-status -M a b`
pub fn name_status(git: &Git, a: &str, b: &str) -> Result<Vec<Change>, GitError> {
    let out = git.run(["diff-tree", "-r", "-z", "--name-status", "-M", a, b])?;
    Ok(parse_name_status(&out))
}

fn parse_name_status(out: &[u8]) -> Vec<Change> {
    let mut tokens = out
        .split(|b| *b == 0)
        .filter(|t| !t.is_empty())
        .map(String::from_utf8_lossy);
    let mut changes = Vec::new();
    while let Some(status) = tokens.next() {
        let code = status.chars().next().unwrap_or('?');
        let path = match code {
            'R' | 'C' => {
                let old = tokens.next().unwrap_or_default();
                let new = tokens.next().unwrap_or_default();
                format!("{old} -> {new}")
            }
            _ => tokens.next().unwrap_or_default().into_owned(),
        };
        changes.push(Change { code, path });
    }
    changes
}

/// How many paths `b` adds on top of `a`.
pub fn count_added(git: &Git, a: &str, b: &str) -> Result<usize, GitError> {
    let out = git.run([
        "diff-tree",
        "-r",
        "-z",
        "--name-only",
        "--diff-filter=A",
        a,
        b,
    ])?;
    Ok(out.split(|b| *b == 0).filter(|t| !t.is_empty()).count())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numstat_handles_plain_binary_and_renamed_files() {
        let out = b"3\t1\tsrc/a.rs\0-\t-\timg.png\x000\t0\t\0old name\0new name\0";
        assert_eq!(
            parse_numstat(out),
            [
                FileStat {
                    added: Some(3),
                    deleted: Some(1),
                    path: "src/a.rs".into()
                },
                FileStat {
                    added: None,
                    deleted: None,
                    path: "img.png".into()
                },
                FileStat {
                    added: Some(0),
                    deleted: Some(0),
                    path: "old name -> new name".into()
                },
            ]
        );
    }

    #[test]
    fn name_status_handles_renames() {
        let out = b"M\0a\0A\0b c\0R100\0old\0new\0D\0gone\0";
        let got: Vec<(char, String)> = parse_name_status(out)
            .into_iter()
            .map(|c| (c.code, c.path))
            .collect();
        assert_eq!(
            got,
            [
                ('M', "a".to_string()),
                ('A', "b c".to_string()),
                ('R', "old -> new".to_string()),
                ('D', "gone".to_string()),
            ]
        );
    }

    #[test]
    fn summary_wording() {
        assert_eq!(summary(&[]), "no changes");
        let one = [FileStat {
            added: Some(5),
            deleted: Some(2),
            path: "a".into(),
        }];
        assert_eq!(summary(&one), "1 file changed, +5 -2");
        let two = [
            FileStat {
                added: Some(5),
                deleted: Some(2),
                path: "a".into(),
            },
            FileStat {
                added: None,
                deleted: None,
                path: "b".into(),
            },
        ];
        assert_eq!(summary(&two), "2 files changed, +5 -2");
    }
}
