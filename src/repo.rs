//! What we know about the repository and the place (branch or detached HEAD) the user is working in.

use std::fmt;
use std::path::{Path, PathBuf};

use crate::git::{Git, GitError};

/// Where a chain of stones lives: one branch, or the detached HEAD of one worktree (I9).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Scope {
    /// The full branch ref, e.g. `refs/heads/feature/auth`.
    Branch(String),
    Detached,
}

impl Scope {
    /// The ref that holds this scope's state commit (A4).
    pub fn state_ref(&self) -> String {
        match self {
            Scope::Branch(full) => {
                format!("refs/stones/{}", full.strip_prefix("refs/").unwrap_or(full))
            }
            Scope::Detached => "refs/worktree/stones/detached".to_string(),
        }
    }
}

#[derive(Debug)]
pub enum RepoError {
    /// Git itself said this is not a repository. Carries git's own message.
    NotARepo(GitError),
    Bare,
    NotInWorkTree,
    Git(GitError),
    Unexpected(String),
}

impl RepoError {
    /// 1 = the user can fix it, 2 = internal or git failure.
    pub fn exit_code(&self) -> i32 {
        match self {
            RepoError::NotARepo(_) | RepoError::Bare | RepoError::NotInWorkTree => 1,
            RepoError::Git(_) | RepoError::Unexpected(_) => 2,
        }
    }
}

impl fmt::Display for RepoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RepoError::NotARepo(e) => write!(f, "{}", e.stderr.trim_end()),
            RepoError::Bare => write!(
                f,
                "this is a bare repository; git stone needs a working tree"
            ),
            RepoError::NotInWorkTree => write!(
                f,
                "not inside a working tree (are you inside the .git directory?)"
            ),
            RepoError::Git(e) => write!(f, "{e}"),
            RepoError::Unexpected(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for RepoError {}

impl From<GitError> for RepoError {
    fn from(e: GitError) -> RepoError {
        RepoError::Git(e)
    }
}

pub struct Repo {
    /// Top of the working tree.
    pub top: PathBuf,
    /// This worktree's git dir (per worktree, not the shared one).
    pub git_dir: PathBuf,
    /// The real index of this worktree.
    pub index_path: PathBuf,
    pub scope: Scope,
    /// `None` on an unborn branch.
    pub head: Option<String>,
    pub empty_tree: String,
}

/// (path asked of `git rev-parse --git-path`, name shown to the user)
const OPERATIONS: [(&str, &str); 6] = [
    ("MERGE_HEAD", "merge"),
    ("CHERRY_PICK_HEAD", "cherry-pick"),
    ("REVERT_HEAD", "revert"),
    ("rebase-merge", "rebase"),
    ("rebase-apply", "rebase or am"),
    ("BISECT_LOG", "bisect"),
];

impl Repo {
    /// Find the repository that contains the current directory.
    pub fn discover() -> Result<Repo, RepoError> {
        let cwd = std::env::current_dir().map_err(|e| {
            RepoError::Unexpected(format!("cannot read the current directory: {e}"))
        })?;
        Repo::discover_in(&cwd)
    }

    pub fn discover_in(start: &Path) -> Result<Repo, RepoError> {
        let probe = Git::new().in_dir(start);
        // Exit 128 means git could not find a repository; its message says why (not a repo, unsafe owner, ...).
        let out = probe
            .run([
                "rev-parse",
                "--path-format=absolute",
                "--is-bare-repository",
                "--is-inside-work-tree",
                "--git-dir",
                "--git-path",
                "index",
            ])
            .map_err(|e| match e.code {
                Some(128) => RepoError::NotARepo(e),
                _ => RepoError::Git(e),
            })?;
        let [bare, inside, git_dir, index_path] = lines(&out)?;
        if bare == "true" {
            return Err(RepoError::Bare);
        }
        if inside != "true" {
            return Err(RepoError::NotInWorkTree);
        }

        let top =
            single(&probe.run(["rev-parse", "--path-format=absolute", "--show-toplevel"])?)?;
        let repo_git = Git::new().in_dir(&top);

        // `symbolic-ref -q` exits 1 on a detached HEAD, and works on an unborn branch.
        let scope = match repo_git.run(["symbolic-ref", "-q", "HEAD"]) {
            Ok(out) => {
                let branch = single(&out)?;
                if !branch.starts_with("refs/heads/") {
                    return Err(RepoError::Unexpected(format!(
                        "HEAD points at `{branch}`, not a branch"
                    )));
                }
                Scope::Branch(branch)
            }
            Err(e) if e.code == Some(1) => Scope::Detached,
            Err(e) => return Err(e.into()),
        };
        // `--verify -q` exits 1 when HEAD has no commit yet.
        let head = match repo_git.run(["rev-parse", "-q", "--verify", "HEAD"]) {
            Ok(out) => Some(single(&out)?),
            Err(e) if e.code == Some(1) => None,
            Err(e) => return Err(e.into()),
        };
        // Not hardcoded: SHA-256 repos have a different empty tree.
        let empty_tree =
            single(&repo_git.run_stdin(["hash-object", "-t", "tree", "--stdin"], b"")?)?;

        Ok(Repo {
            top: PathBuf::from(top),
            git_dir: PathBuf::from(git_dir),
            index_path: PathBuf::from(index_path),
            scope,
            head,
            empty_tree,
        })
    }

    /// The all-zero id git uses for "this ref must not exist yet". Its length depends on the object format.
    pub fn zero_oid(&self) -> String {
        "0".repeat(self.empty_tree.len())
    }

    /// A git runner that starts at the top of the working tree.
    pub fn git(&self) -> Git {
        Git::new().in_dir(&self.top)
    }

    /// The name of the merge, rebase, ... that is still running, if any.
    pub fn op_in_progress(&self) -> Result<Option<&'static str>, RepoError> {
        let mut args = vec!["rev-parse", "--path-format=absolute"];
        for (path, _) in OPERATIONS {
            args.extend(["--git-path", path]);
        }
        let out = self.git().run(args)?;
        let paths = lines::<{ OPERATIONS.len() }>(&out)?;
        let running = OPERATIONS
            .iter()
            .zip(paths)
            .find(|(_, path)| Path::new(path).exists())
            .map(|((_, name), _)| *name);
        Ok(running)
    }

    /// True when the index has conflicted (unmerged) entries.
    pub fn has_unmerged(&self) -> Result<bool, RepoError> {
        // Runs at the top, because `ls-files` only looks below the current directory.
        Ok(!self.git().run(["ls-files", "-u", "-z"])?.is_empty())
    }
}

/// Split git output into exactly `N` lines. Anything else is refused rather than guessed at.
fn lines<const N: usize>(out: &[u8]) -> Result<[&str; N], RepoError> {
    let text = std::str::from_utf8(out)
        .map_err(|_| RepoError::Unexpected("git printed a path that is not valid UTF-8".into()))?;
    let text = text.strip_suffix('\n').unwrap_or(text);
    text.split('\n')
        .collect::<Vec<_>>()
        .try_into()
        .map_err(|_| {
            RepoError::Unexpected(format!(
                "unexpected git output (paths containing a newline are not supported): {text:?}"
            ))
        })
}

fn single(out: &[u8]) -> Result<String, RepoError> {
    Ok(lines::<1>(out)?[0].to_string())
}

/// Fixture helpers shared by the unit tests of several modules.
#[cfg(test)]
pub(crate) mod testutil {
    use super::*;
    use std::fs;

    /// Git with a fixed identity and no user or system config, for building fixtures.
    pub fn fixture_git(dir: &Path) -> Git {
        Git::new()
            .in_dir(dir)
            .with_env("GIT_CONFIG_GLOBAL", "/dev/null")
            .with_env("GIT_CONFIG_NOSYSTEM", "1")
            .with_env("GIT_AUTHOR_NAME", "Test")
            .with_env("GIT_AUTHOR_EMAIL", "test@example.com")
            .with_env("GIT_COMMITTER_NAME", "Test")
            .with_env("GIT_COMMITTER_EMAIL", "test@example.com")
            .with_env("HOME", dir)
    }

    pub fn init(dir: &Path) -> Git {
        let git = fixture_git(dir);
        git.run(["init", "-q", "-b", "main"]).unwrap();
        git
    }

    pub fn commit_file(git: &Git, dir: &Path, name: &str, text: &str) {
        fs::write(dir.join(name), text).unwrap();
        git.run(["add", name]).unwrap();
        git.run(["commit", "-q", "-m", name]).unwrap();
    }
}

#[cfg(test)]
mod tests {
    use super::testutil::{commit_file, fixture_git, init};
    use super::*;
    use std::fs;

    #[test]
    fn state_ref_mirrors_the_branch_ref() {
        let nested = Scope::Branch("refs/heads/feature/auth".into());
        assert_eq!(nested.state_ref(), "refs/stones/heads/feature/auth");
        assert_eq!(Scope::Detached.state_ref(), "refs/worktree/stones/detached");
    }

    #[test]
    fn unborn_branch() {
        let dir = tempfile::tempdir().unwrap();
        init(dir.path());
        let repo = Repo::discover_in(dir.path()).unwrap();
        assert_eq!(repo.scope, Scope::Branch("refs/heads/main".into()));
        assert_eq!(repo.head, None);
        assert_eq!(repo.empty_tree.len(), 40);
        assert_eq!(repo.empty_tree, "4b825dc642cb6eb9a060e54bf8d69288fbee4904");
        let top = dir.path().canonicalize().unwrap();
        assert_eq!(repo.top, top);
        assert_eq!(repo.git_dir, top.join(".git"));
        assert_eq!(repo.index_path, top.join(".git/index"));
    }

    #[test]
    fn born_branch_and_subdirectory_start() {
        let dir = tempfile::tempdir().unwrap();
        let git = init(dir.path());
        commit_file(&git, dir.path(), "a", "a");
        fs::create_dir(dir.path().join("sub")).unwrap();
        let repo = Repo::discover_in(&dir.path().join("sub")).unwrap();
        assert_eq!(repo.top, dir.path().canonicalize().unwrap());
        let head = String::from_utf8(git.run(["rev-parse", "HEAD"]).unwrap()).unwrap();
        assert_eq!(repo.head.as_deref(), Some(head.trim_end()));
    }

    #[test]
    fn detached_head() {
        let dir = tempfile::tempdir().unwrap();
        let git = init(dir.path());
        commit_file(&git, dir.path(), "a", "a");
        git.run(["checkout", "-q", "--detach"]).unwrap();
        let repo = Repo::discover_in(dir.path()).unwrap();
        assert_eq!(repo.scope, Scope::Detached);
        assert!(repo.head.is_some());
    }

    #[test]
    fn sha256_repo() {
        let dir = tempfile::tempdir().unwrap();
        fixture_git(dir.path())
            .run(["init", "-q", "-b", "main", "--object-format=sha256"])
            .unwrap();
        let repo = Repo::discover_in(dir.path()).unwrap();
        assert_eq!(repo.empty_tree.len(), 64);
    }

    #[test]
    fn worktree_has_its_own_git_dir_and_index() {
        let dir = tempfile::tempdir().unwrap();
        let main = dir.path().join("main");
        let other = dir.path().join("other");
        fs::create_dir(&main).unwrap();
        let git = init(&main);
        commit_file(&git, &main, "a", "a");
        git.run([
            "worktree",
            "add",
            "-q",
            "-b",
            "side",
            other.to_str().unwrap(),
        ])
        .unwrap();
        let repo = Repo::discover_in(&other).unwrap();
        assert_eq!(repo.scope, Scope::Branch("refs/heads/side".into()));
        assert_eq!(repo.top, other.canonicalize().unwrap());
        assert_ne!(repo.git_dir, main.canonicalize().unwrap().join(".git"));
        assert_eq!(repo.index_path, repo.git_dir.join("index"));
    }

    #[test]
    fn bare_repo_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        fixture_git(dir.path())
            .run(["init", "-q", "--bare"])
            .unwrap();
        let err = Repo::discover_in(dir.path()).err().unwrap();
        assert!(matches!(err, RepoError::Bare));
        assert_eq!(err.exit_code(), 1);
    }

    #[test]
    fn inside_the_git_dir_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        init(dir.path());
        let err = Repo::discover_in(&dir.path().join(".git")).err().unwrap();
        assert!(matches!(err, RepoError::NotInWorkTree));
    }

    #[test]
    fn not_a_repo_shows_gits_message() {
        // The temp dir is not inside any repository.
        let dir = tempfile::tempdir().unwrap();
        let err = Repo::discover_in(dir.path()).err().unwrap();
        assert!(matches!(err, RepoError::NotARepo(_)));
        assert_eq!(err.exit_code(), 1);
        assert!(!err.to_string().is_empty());
    }

    #[test]
    fn operations_and_unmerged_paths() {
        let dir = tempfile::tempdir().unwrap();
        let git = init(dir.path());
        commit_file(&git, dir.path(), "f", "base\n");
        let repo = Repo::discover_in(dir.path()).unwrap();
        assert_eq!(repo.op_in_progress().unwrap(), None);
        assert!(!repo.has_unmerged().unwrap());

        // A real conflicting merge leaves MERGE_HEAD and an unmerged entry.
        git.run(["checkout", "-q", "-b", "topic"]).unwrap();
        commit_file(&git, dir.path(), "f", "topic\n");
        git.run(["checkout", "-q", "main"]).unwrap();
        commit_file(&git, dir.path(), "f", "main\n");
        assert!(git.run(["merge", "-q", "topic"]).is_err());
        assert_eq!(repo.op_in_progress().unwrap(), Some("merge"));
        assert!(repo.has_unmerged().unwrap());

        git.run(["merge", "--abort"]).unwrap();
        assert_eq!(repo.op_in_progress().unwrap(), None);
        fs::create_dir(repo.git_dir.join("rebase-merge")).unwrap();
        assert_eq!(repo.op_in_progress().unwrap(), Some("rebase"));
    }
}
