//! Where a stone sits in its chain: its base, the latest tree, and tree-to-tree change lists.
//!
//! Also the branch-movement rules (A9): classify where HEAD is, and auto-reconcile the safe case.

use crate::git::{Git, GitError};
use crate::repo::Repo;
use crate::state::{State, StateError};

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

/// Where HEAD stands compared with the commit the stones were made on (A9).
#[derive(Debug, PartialEq, Eq)]
pub enum Movement {
    /// Nothing to worry about (also: no stones, so nothing can be misattributed).
    InSync,
    /// HEAD only has commits that are exactly the first `n` stones, in order.
    Matches(usize),
    /// Anything else: an ordinary commit, pull, rebase, reset, amend, a recreated branch.
    Moved,
}

/// Cheap: when HEAD is the anchor it is one comparison, otherwise one ancestor check and one bounded `git log`.
pub fn classify(repo: &Repo, state: &State) -> Result<Movement, GitError> {
    if state.stones.is_empty() || repo.head == state.anchor {
        return Ok(Movement::InSync);
    }
    let Some(head) = repo.head.as_deref() else {
        return Ok(Movement::Moved);
    };
    let git = repo.git();
    // An unborn anchor counts as an ancestor of everything.
    let range = match &state.anchor {
        Some(anchor) => {
            match git.run(["merge-base", "--is-ancestor", anchor, head]) {
                Ok(_) => {}
                Err(e) if e.code == Some(1) => return Ok(Movement::Moved),
                Err(e) => return Err(e),
            }
            format!("{anchor}..{head}")
        }
        None => head.to_string(),
    };
    // One more than the stones: if that many commits exist, they cannot all match.
    let limit = (state.stones.len() + 1).to_string();
    let log = git.run([
        "log",
        "--first-parent",
        "--reverse",
        "-z",
        "-n",
        &limit,
        "--format=%H%x09%P%x09%T",
        &range,
    ])?;

    let mut previous = state.anchor.clone();
    let mut count = 0;
    for record in log.split(|b| *b == 0).filter(|r| !r.is_empty()) {
        let text = String::from_utf8_lossy(record);
        let mut fields = text.split('\t');
        let (Some(oid), Some(parents), Some(tree)) = (fields.next(), fields.next(), fields.next())
        else {
            return Ok(Movement::Moved);
        };
        // Each commit must sit directly on the one before it (the first on the anchor) and not be a merge.
        let parents: Vec<&str> = parents.split(' ').filter(|p| !p.is_empty()).collect();
        let expected: Vec<&str> = previous.as_deref().into_iter().collect();
        let Some(stone) = state.stones.get(count) else {
            return Ok(Movement::Moved);
        };
        if parents != expected || stone.tree != tree {
            return Ok(Movement::Moved);
        }
        previous = Some(oid.to_string());
        count += 1;
    }
    Ok(if count == 0 {
        Movement::Moved
    } else {
        Movement::Matches(count)
    })
}

pub enum SyncError {
    Git(GitError),
    /// The staged changes in the real index cannot be moved forward onto the commits.
    Index(GitError),
    State(StateError),
}

impl From<GitError> for SyncError {
    fn from(e: GitError) -> SyncError {
        SyncError::Git(e)
    }
}

impl From<StateError> for SyncError {
    fn from(e: StateError) -> SyncError {
        SyncError::State(e)
    }
}

pub struct Synced {
    pub oid: String,
    pub state: State,
    /// For the user: which stones were found already committed.
    pub note: String,
}

pub enum Outcome {
    Unchanged,
    Reconciled(Synced),
    Moved,
}

/// Auto-reconcile (A9). When the branch only holds commits that equal the first stones, bring the
/// real index forward one step per stone (`read-tree -m <prev> <next>`, safe to repeat) and record
/// those stones as committed with one compare-and-swap. This also heals an interrupted commit.
pub fn sync(repo: &Repo, oid: &str, state: &State) -> Result<Outcome, SyncError> {
    let count = match classify(repo, state)? {
        Movement::InSync => return Ok(Outcome::Unchanged),
        Movement::Moved => return Ok(Outcome::Moved),
        Movement::Matches(count) => count,
    };
    let git = repo.git().allow_optional_locks();
    let mut previous = anchor_tree(repo, state)?;
    for stone in &state.stones[..count] {
        git.run(["read-tree", "-m", "-i", &previous, &stone.tree])
            .map_err(SyncError::Index)?;
        previous = stone.tree.clone();
    }

    let mut next = state.clone();
    let done: Vec<u64> = next.stones.drain(..count).map(|s| s.id).collect();
    next.anchor = repo.head.clone();
    let note = format!(
        "Note: {} match{} already on the branch; marked as committed.",
        describe_ids(&done),
        if done.len() == 1 {
            "es a commit"
        } else {
            " commits"
        }
    );
    let new_oid = next.save(repo, Some(oid), &format!("reconcile {}", done.len()))?;
    Ok(Outcome::Reconciled(Synced {
        oid: new_oid,
        state: next,
        note,
    }))
}

/// `Stone 3`, `Stones 1–2`, or `Stones 1, 3` when some id in between is gone.
pub fn describe_ids(ids: &[u64]) -> String {
    match ids {
        [one] => format!("Stone {one}"),
        [first, .., last] if ids.windows(2).all(|w| w[1] == w[0] + 1) => {
            format!("Stones {first}–{last}")
        }
        _ => format!(
            "Stones {}",
            ids.iter()
                .map(u64::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

/// The oldest Git whose `merge-tree` has `--write-tree` together with `--merge-base` (6.1).
pub const REPAIR_MIN_GIT: (u32, u32) = (2, 40);

/// `git version 2.53.0` or `git version 2.39.5 (Apple Git-154)` -> (2, 39).
fn parse_git_version(text: &str) -> Option<(u32, u32)> {
    let version = text
        .trim()
        .strip_prefix("git version ")?
        .split(' ')
        .next()?;
    let mut parts = version.split('.');
    Some((parts.next()?.parse().ok()?, parts.next()?.parse().ok()?))
}

/// Checked only by `repair`, the one command that needs this Git.
pub fn require_repair_git() -> Result<(), String> {
    let out = Git::new()
        .run(["version"])
        .map_err(|e| format!("cannot run git: {e}"))?;
    let text = String::from_utf8_lossy(&out);
    match parse_git_version(&text) {
        Some(found) if found >= REPAIR_MIN_GIT => Ok(()),
        _ => Err(format!(
            "`git stone repair` needs Git {}.{} or newer (found: {}).",
            REPAIR_MIN_GIT.0,
            REPAIR_MIN_GIT.1,
            text.trim()
        )),
    }
}

/// What replaying the stones onto the current HEAD gives (A9).
#[derive(Debug, PartialEq, Eq)]
pub enum Replay {
    /// A new tree for every stone, in chain order.
    Clean(Vec<String>),
    /// The stone at this position cannot be put on top of HEAD. Later stones are not tried.
    Conflict { index: usize, paths: Vec<String> },
}

/// Git's 3-way merge for each stone in turn: base = where the stone started, ours = where it
/// would start now, theirs = the stone. Writes only objects; no ref, index or file changes.
pub fn replay(repo: &Repo, state: &State) -> Result<Replay, GitError> {
    let git = repo.git();
    let mut old_base = anchor_tree(repo, state)?;
    let mut new_base = head_tree(repo)?;
    let mut trees = Vec::with_capacity(state.stones.len());
    for (index, stone) in state.stones.iter().enumerate() {
        let (code, out) = git.run_answers(
            [
                "merge-tree",
                "--write-tree",
                "--name-only",
                "-z",
                &format!("--merge-base={old_base}"),
                &new_base,
                &stone.tree,
            ],
            &[1],
        )?;
        let (tree, paths) = parse_merge_tree(&out);
        if code == 1 {
            return Ok(Replay::Conflict { index, paths });
        }
        old_base = stone.tree.clone();
        new_base = tree.clone();
        trees.push(tree);
    }
    Ok(Replay::Clean(trees))
}

/// `git merge-tree --write-tree --name-only -z`: `<tree>\0`, then on a conflict each conflicted
/// path and an empty token, then informational messages that are not needed here.
fn parse_merge_tree(out: &[u8]) -> (String, Vec<String>) {
    let mut tokens = out.split(|b| *b == 0);
    let tree = tokens
        .next()
        .map(|t| String::from_utf8_lossy(t).into_owned())
        .unwrap_or_default();
    let paths = tokens
        .take_while(|t| !t.is_empty())
        .map(|t| String::from_utf8_lossy(t).into_owned())
        .collect();
    (tree, paths)
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
    fn merge_tree_output_is_split_into_tree_and_conflicted_paths() {
        assert_eq!(
            parse_merge_tree(b"abc123\0"),
            ("abc123".to_string(), vec![])
        );
        let conflict =
            b"abc123\0src/a b.rs\0f\0\x001\0f\0CONFLICT (contents)\0Merge conflict in f\n\0";
        assert_eq!(
            parse_merge_tree(conflict),
            ("abc123".to_string(), vec!["src/a b.rs".into(), "f".into()])
        );
    }

    #[test]
    fn git_versions_are_read_from_the_first_line() {
        assert_eq!(parse_git_version("git version 2.53.0\n"), Some((2, 53)));
        assert_eq!(
            parse_git_version("git version 2.39.5 (Apple Git-154)\n"),
            Some((2, 39))
        );
        assert_eq!(
            parse_git_version("git version 2.40.0.windows.1"),
            Some((2, 40))
        );
        assert_eq!(parse_git_version("something else"), None);
        assert!(require_repair_git().is_ok());
    }

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
    fn id_lists_read_naturally() {
        assert_eq!(describe_ids(&[3]), "Stone 3");
        assert_eq!(describe_ids(&[1, 2]), "Stones 1–2");
        assert_eq!(describe_ids(&[4, 5, 6]), "Stones 4–6");
        assert_eq!(describe_ids(&[1, 3]), "Stones 1, 3");
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
