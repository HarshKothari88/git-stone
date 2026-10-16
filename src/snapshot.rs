//! Turn the working tree into a tree object without touching the real index (A5).
//!
//! The recipe: copy the real index to a temp file, run `git add -A` and `git write-tree` against
//! the copy, delete the copy. Git itself does the hard parts (.gitignore, attributes, clean filters,
//! modes, symlinks, submodules), and the stat cache in the copy means only changed files are hashed.

use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::git::GitError;
use crate::repo::Repo;

const PREFIX: &str = "stone-index-";
const STALE_AFTER: Duration = Duration::from_secs(3600);

/// Two snapshots in one process (tests, or later parallel work) must not share a file name.
static COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug)]
pub enum SnapshotError {
    Io(io::Error),
    Git(GitError),
}

impl SnapshotError {
    pub fn exit_code(&self) -> i32 {
        2
    }
}

impl fmt::Display for SnapshotError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SnapshotError::Io(e) => write!(f, "cannot copy the index for a snapshot: {e}"),
            SnapshotError::Git(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for SnapshotError {}

impl From<io::Error> for SnapshotError {
    fn from(e: io::Error) -> SnapshotError {
        SnapshotError::Io(e)
    }
}

impl From<GitError> for SnapshotError {
    fn from(e: GitError) -> SnapshotError {
        SnapshotError::Git(e)
    }
}

/// A copy of the real index that is deleted when this value is dropped, also on error.
pub struct TempIndex {
    path: PathBuf,
}

impl TempIndex {
    /// Make the copy inside this worktree's git dir: same filesystem, and a split index finds its `sharedindex.*` files.
    pub fn create(repo: &Repo) -> Result<TempIndex, SnapshotError> {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let name = format!("{PREFIX}{}-{nanos}-{n}", std::process::id());
        // The guard exists before the copy, so a half-written copy is removed too.
        let temp = TempIndex {
            path: repo.git_dir.join(name),
        };
        // Copying keeps the stat cache, skip-worktree and assume-unchanged bits and intent-to-add entries.
        // No real index yet (fresh repo) means the copy starts empty.
        match fs::copy(&repo.index_path, &temp.path) {
            Ok(_) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
        Ok(temp)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempIndex {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
        let mut lock = self.path.clone().into_os_string();
        lock.push(".lock");
        let _ = fs::remove_file(lock);
    }
}

/// The tree that `git add -A && git write-tree` would make, with the real index left byte for byte as it was.
pub fn snapshot(repo: &Repo) -> Result<String, SnapshotError> {
    let temp = TempIndex::create(repo)?;
    // Runs at the top of the working tree with no pathspec, so the whole tree is seen.
    let git = repo.git().with_index(temp.path());
    git.run(["add", "-A"])?;
    let tree = git.run(["write-tree"])?;
    Ok(String::from_utf8_lossy(&tree).trim().to_string())
}

/// Temp indexes that a killed process left behind (older than one hour).
pub fn stale_indexes(repo: &Repo) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(&repo.git_dir) else {
        return Vec::new();
    };
    let now = SystemTime::now();
    let mut stale = Vec::new();
    for entry in entries.flatten() {
        if !entry.file_name().to_string_lossy().starts_with(PREFIX) {
            continue;
        }
        let age = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|modified| now.duration_since(modified).ok());
        if age.is_some_and(|age| age > STALE_AFTER) {
            stale.push(entry.path());
        }
    }
    stale
}

/// Delete the stale temp indexes. Best effort.
pub fn clean_stale(repo: &Repo) {
    for path in stale_indexes(repo) {
        let _ = fs::remove_file(path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::Git;
    use crate::repo::testutil::{commit_file, fixture_git, init};
    use std::process::Command;
    use tempfile::TempDir;

    fn write(dir: &Path, name: &str, bytes: &[u8]) {
        fs::write(dir.join(name), bytes).unwrap();
    }

    fn oid(out: Vec<u8>) -> String {
        String::from_utf8(out).unwrap().trim().to_string()
    }

    /// Every path in a tree.
    fn names(git: &Git, tree: &str) -> Vec<String> {
        let out = git
            .run(["ls-tree", "-r", "-z", "--name-only", tree])
            .unwrap();
        String::from_utf8(out)
            .unwrap()
            .split('\0')
            .filter(|n| !n.is_empty())
            .map(String::from)
            .collect()
    }

    fn blob(git: &Git, tree: &str, path: &str) -> Vec<u8> {
        git.run(["cat-file", "blob", &format!("{tree}:{path}")])
            .unwrap()
    }

    /// `<mode> <type> <oid>\t<path>` of one path.
    fn entry(git: &Git, tree: &str, path: &str) -> String {
        let out = git.run(["ls-tree", tree, "--", path]).unwrap();
        String::from_utf8(out).unwrap().trim_end().to_string()
    }

    fn leftovers(repo: &Repo) -> Vec<String> {
        fs::read_dir(&repo.git_dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with(PREFIX))
            .collect()
    }

    /// A full copy of a repository, including `.git`, so the reference starts from the same state.
    fn copy_dir(from: &Path, to: &Path) {
        let status = Command::new("cp")
            .arg("-Rp")
            .arg(from.join("."))
            .arg(to)
            .status()
            .unwrap();
        assert!(status.success());
    }

    struct Checked {
        dir: TempDir,
        git: Git,
        tree: String,
    }

    /// Build a repo with `setup`, snapshot it, and check three things:
    /// the tree equals `git add -A && git write-tree` run in a separate copy, the real index is
    /// byte-identical afterwards (or still missing), and no temp index is left behind.
    fn check_raw(setup: impl FnOnce(&Path, &Git)) -> Checked {
        let dir = tempfile::tempdir().unwrap();
        let git = init(dir.path());
        setup(dir.path(), &git);
        let repo = Repo::discover_in(dir.path()).unwrap();

        let reference = tempfile::tempdir().unwrap();
        copy_dir(dir.path(), reference.path());
        let ref_git = fixture_git(reference.path());
        ref_git.run(["add", "-A"]).unwrap();
        let expected = oid(ref_git.run(["write-tree"]).unwrap());

        let before = fs::read(&repo.index_path).ok();
        let tree = snapshot(&repo).unwrap();
        assert_eq!(
            fs::read(&repo.index_path).ok(),
            before,
            "the real index changed"
        );
        assert_eq!(leftovers(&repo), Vec::<String>::new());
        assert_eq!(
            tree, expected,
            "differs from `git add -A && git write-tree`"
        );
        Checked { dir, git, tree }
    }

    /// Same, on a repo that already has one commit with `base.txt`.
    fn check(setup: impl FnOnce(&Path, &Git)) -> Checked {
        check_raw(|dir, git| {
            commit_file(git, dir, "base.txt", "base\n");
            setup(dir, git);
        })
    }

    #[test]
    fn modified_new_and_deleted_files() {
        let c = check(|d, _| {
            write(d, "base.txt", b"changed\n");
            write(d, "new.txt", b"new\n");
        });
        assert_eq!(blob(&c.git, &c.tree, "base.txt"), b"changed\n");
        assert_eq!(blob(&c.git, &c.tree, "new.txt"), b"new\n");

        let c = check(|d, _| fs::remove_file(d.join("base.txt")).unwrap());
        assert!(names(&c.git, &c.tree).is_empty());
    }

    #[test]
    fn renamed_file_is_just_a_delete_and_an_add() {
        let c = check(|d, _| fs::rename(d.join("base.txt"), d.join("moved.txt")).unwrap());
        assert_eq!(names(&c.git, &c.tree), ["moved.txt"]);
    }

    #[test]
    fn binary_file() {
        let bytes = [0u8, 1, 2, 255, 0, 10, 13, 0];
        let c = check(|d, _| write(d, "bin.dat", &bytes));
        assert_eq!(blob(&c.git, &c.tree, "bin.dat"), bytes);
    }

    #[cfg(unix)]
    #[test]
    fn symlink() {
        let c = check(|d, _| std::os::unix::fs::symlink("base.txt", d.join("link")).unwrap());
        assert!(entry(&c.git, &c.tree, "link").starts_with("120000 blob"));
        assert_eq!(blob(&c.git, &c.tree, "link"), b"base.txt");
    }

    #[cfg(unix)]
    #[test]
    fn executable_bit() {
        use std::os::unix::fs::PermissionsExt;
        let c = check(|d, _| {
            fs::set_permissions(d.join("base.txt"), fs::Permissions::from_mode(0o755)).unwrap()
        });
        assert!(entry(&c.git, &c.tree, "base.txt").starts_with("100755 blob"));
    }

    #[test]
    fn ignored_file_is_absent() {
        let c = check(|d, g| {
            write(d, ".gitignore", b"ignored.txt\nbuild/\n");
            g.run(["add", ".gitignore"]).unwrap();
            g.run(["commit", "-q", "-m", "ignore"]).unwrap();
            write(d, "ignored.txt", b"x\n");
            fs::create_dir(d.join("build")).unwrap();
            write(d, "build/out.o", b"x\n");
            write(d, "kept.txt", b"y\n");
        });
        assert_eq!(
            names(&c.git, &c.tree),
            [".gitignore", "base.txt", "kept.txt"]
        );
    }

    #[test]
    fn staged_and_unstaged_changes_to_one_file_worktree_wins() {
        let c = check(|d, g| {
            write(d, "base.txt", b"staged\n");
            g.run(["add", "base.txt"]).unwrap();
            write(d, "base.txt", b"unstaged\n");
        });
        assert_eq!(blob(&c.git, &c.tree, "base.txt"), b"unstaged\n");
    }

    #[test]
    fn intent_to_add_entry() {
        let c = check(|d, g| {
            write(d, "new.txt", b"n\n");
            g.run(["add", "-N", "new.txt"]).unwrap();
        });
        assert_eq!(blob(&c.git, &c.tree, "new.txt"), b"n\n");
    }

    #[test]
    fn assume_unchanged_keeps_the_index_version() {
        let c = check(|d, g| {
            g.run(["update-index", "--assume-unchanged", "base.txt"])
                .unwrap();
            write(d, "base.txt", b"edited behind git's back\n");
        });
        assert_eq!(blob(&c.git, &c.tree, "base.txt"), b"base\n");
    }

    #[test]
    fn skip_worktree_keeps_the_index_version() {
        let c = check(|d, g| {
            g.run(["update-index", "--skip-worktree", "base.txt"])
                .unwrap();
            write(d, "base.txt", b"edited\n");
        });
        assert_eq!(blob(&c.git, &c.tree, "base.txt"), b"base\n");
    }

    fn sparse(sparse_index: bool) {
        let c = check_raw(|d, g| {
            fs::create_dir(d.join("a")).unwrap();
            fs::create_dir(d.join("b")).unwrap();
            write(d, "a/x.txt", b"x\n");
            write(d, "b/y.txt", b"y\n");
            g.run(["add", "-A"]).unwrap();
            g.run(["commit", "-q", "-m", "two dirs"]).unwrap();
            let mut args = vec!["sparse-checkout", "set", "--cone"];
            args.push(if sparse_index {
                "--sparse-index"
            } else {
                "--no-sparse-index"
            });
            args.push("a");
            g.run(args).unwrap();
            assert!(!d.join("b").exists(), "b/ should be out of the cone");
            write(d, "a/x.txt", b"changed\n");
            write(d, "a/new.txt", b"n\n");
        });
        // Out-of-cone files are neither dropped nor reported as deleted.
        assert_eq!(blob(&c.git, &c.tree, "b/y.txt"), b"y\n");
        assert_eq!(blob(&c.git, &c.tree, "a/x.txt"), b"changed\n");
        assert_eq!(blob(&c.git, &c.tree, "a/new.txt"), b"n\n");
    }

    #[test]
    fn sparse_checkout_keeps_out_of_cone_entries() {
        sparse(false);
    }

    #[test]
    fn sparse_checkout_with_sparse_index() {
        sparse(true);
    }

    #[test]
    fn split_index() {
        let c = check(|d, g| {
            g.run(["update-index", "--split-index"]).unwrap();
            let shared = fs::read_dir(d.join(".git")).unwrap().any(|e| {
                e.unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with("sharedindex.")
            });
            assert!(shared, "the scenario needs a shared index file");
            write(d, "base.txt", b"changed\n");
            write(d, "new.txt", b"n\n");
        });
        assert_eq!(blob(&c.git, &c.tree, "base.txt"), b"changed\n");
    }

    #[test]
    fn submodule_is_recorded_at_its_checked_out_commit() {
        let source = tempfile::tempdir().unwrap();
        let source_git = init(source.path());
        commit_file(&source_git, source.path(), "s.txt", "s\n");
        let c = check(|d, g| {
            g.run([
                "-c",
                "protocol.file.allow=always",
                "submodule",
                "add",
                "-q",
                source.path().to_str().unwrap(),
                "sub",
            ])
            .unwrap();
            g.run(["commit", "-q", "-m", "add sub"]).unwrap();
            // A new commit inside the submodule moves the gitlink; an uncommitted file inside is not captured.
            let sub = d.join("sub");
            commit_file(&fixture_git(&sub), &sub, "s2.txt", "2\n");
            write(&sub, "dirty.txt", b"not captured\n");
        });
        let sub_head = oid(fixture_git(&c.dir.path().join("sub"))
            .run(["rev-parse", "HEAD"])
            .unwrap());
        assert_eq!(
            entry(&c.git, &c.tree, "sub"),
            format!("160000 commit {sub_head}\tsub")
        );
        assert!(!names(&c.git, &c.tree).iter().any(|n| n.contains("dirty")));
    }

    #[test]
    fn crlf_is_converted_with_autocrlf() {
        let c = check(|d, g| {
            g.run(["config", "core.autocrlf", "true"]).unwrap();
            write(d, "crlf.txt", b"a\r\nb\r\n");
            write(d, "base.txt", b"base\r\nmore\r\n");
        });
        assert_eq!(blob(&c.git, &c.tree, "crlf.txt"), b"a\nb\n");
        assert_eq!(blob(&c.git, &c.tree, "base.txt"), b"base\nmore\n");
    }

    #[test]
    fn crlf_is_converted_with_text_auto_attribute() {
        let c = check(|d, g| {
            write(d, ".gitattributes", b"* text=auto\n");
            g.run(["add", ".gitattributes"]).unwrap();
            g.run(["commit", "-q", "-m", "attributes"]).unwrap();
            write(d, "crlf.txt", b"a\r\nb\r\n");
        });
        assert_eq!(blob(&c.git, &c.tree, "crlf.txt"), b"a\nb\n");
    }

    #[test]
    fn awkward_file_names() {
        let awkward = [
            "with space.txt",
            "tab\there.txt",
            "new\nline.txt",
            "ünï-🪨.txt",
            "-dash.txt",
            "$(touch pwned).txt",
            "a;b&c|d.txt",
            "'quote\".txt",
        ];
        let c = check(|d, _| {
            for name in awkward {
                write(d, name, name.as_bytes());
            }
        });
        let found = names(&c.git, &c.tree);
        for name in awkward {
            assert!(found.iter().any(|n| n == name), "missing {name:?}");
            assert_eq!(blob(&c.git, &c.tree, name), name.as_bytes());
        }
        assert!(!c.dir.path().join("pwned").exists());
    }

    #[test]
    fn locked_real_index_does_not_matter() {
        let c = check(|d, _| write(d, "new.txt", b"n\n"));
        let repo = Repo::discover_in(c.dir.path()).unwrap();
        let lock = repo.git_dir.join("index.lock");
        fs::write(&lock, b"held by someone else").unwrap();
        assert_eq!(snapshot(&repo).unwrap(), c.tree);
        assert_eq!(fs::read(&lock).unwrap(), b"held by someone else");
    }

    #[test]
    fn unborn_branch_without_any_index_file() {
        let c = check_raw(|d, _| write(d, "first.txt", b"1\n"));
        let repo = Repo::discover_in(c.dir.path()).unwrap();
        assert!(
            !repo.index_path.exists(),
            "snapshot must not create the real index"
        );
        assert_eq!(names(&c.git, &c.tree), ["first.txt"]);
    }

    #[test]
    fn linked_worktree_uses_its_own_index() {
        let dir = tempfile::tempdir().unwrap();
        let main = dir.path().join("main");
        let other = dir.path().join("other");
        fs::create_dir(&main).unwrap();
        let git = init(&main);
        commit_file(&git, &main, "base.txt", "base\n");
        git.run([
            "worktree",
            "add",
            "-q",
            "-b",
            "side",
            other.to_str().unwrap(),
        ])
        .unwrap();
        write(&other, "base.txt", b"changed in the worktree\n");
        write(&other, "new.txt", b"n\n");

        let repo = Repo::discover_in(&other).unwrap();
        let main_repo = Repo::discover_in(&main).unwrap();
        assert_ne!(repo.git_dir, main_repo.git_dir);
        let temp = TempIndex::create(&repo).unwrap();
        assert_eq!(temp.path().parent().unwrap(), repo.git_dir);
        drop(temp);

        let (before, main_before) = (
            fs::read(&repo.index_path).ok(),
            fs::read(&main_repo.index_path).ok(),
        );
        let tree = snapshot(&repo).unwrap();
        assert_eq!(fs::read(&repo.index_path).ok(), before);
        assert_eq!(fs::read(&main_repo.index_path).ok(), main_before);
        assert_eq!(leftovers(&repo), Vec::<String>::new());

        // The worktree is untouched, so the reference can run on it afterwards.
        let wt_git = fixture_git(&other);
        wt_git.run(["add", "-A"]).unwrap();
        assert_eq!(tree, oid(wt_git.run(["write-tree"]).unwrap()));
    }

    #[test]
    fn git_index_file_is_what_git_path_reports() {
        // VERIFY (A5.1): `--git-path index` follows a user-set GIT_INDEX_FILE.
        let dir = tempfile::tempdir().unwrap();
        let git = init(dir.path());
        let custom = dir.path().canonicalize().unwrap().join("custom-index");
        let out = git
            .with_env("GIT_INDEX_FILE", &custom)
            .run(["rev-parse", "--path-format=absolute", "--git-path", "index"])
            .unwrap();
        assert_eq!(oid(out), custom.to_str().unwrap());
    }

    #[test]
    fn failure_leaves_nothing_behind() {
        let dir = tempfile::tempdir().unwrap();
        let git = init(dir.path());
        commit_file(&git, dir.path(), "base.txt", "base\n");
        let repo = Repo::discover_in(dir.path()).unwrap();
        fs::write(&repo.index_path, b"this is not an index").unwrap();
        let err = snapshot(&repo).unwrap_err();
        assert_eq!(err.exit_code(), 2);
        assert!(matches!(err, SnapshotError::Git(_)), "{err:?}");
        assert_eq!(leftovers(&repo), Vec::<String>::new());
        assert_eq!(fs::read(&repo.index_path).unwrap(), b"this is not an index");
    }

    #[test]
    fn temp_index_is_a_copy_and_dies_with_its_guard() {
        let dir = tempfile::tempdir().unwrap();
        let git = init(dir.path());
        commit_file(&git, dir.path(), "base.txt", "base\n");
        let repo = Repo::discover_in(dir.path()).unwrap();
        let temp = TempIndex::create(&repo).unwrap();
        let path = temp.path().to_path_buf();
        assert_eq!(
            fs::read(&path).unwrap(),
            fs::read(&repo.index_path).unwrap()
        );
        let other = TempIndex::create(&repo).unwrap();
        assert_ne!(other.path(), path, "two guards must not share a file");
        fs::write(format!("{}.lock", path.display()), b"").unwrap();
        drop(temp);
        assert!(!path.exists());
        assert!(!Path::new(&format!("{}.lock", path.display())).exists());
    }

    #[test]
    fn stale_temp_indexes_are_cleaned_after_an_hour() {
        let dir = tempfile::tempdir().unwrap();
        init(dir.path());
        let repo = Repo::discover_in(dir.path()).unwrap();
        let old_time = SystemTime::now() - Duration::from_secs(2 * 3600);
        let make = |name: &str, old: bool| {
            let path = repo.git_dir.join(name);
            fs::write(&path, b"x").unwrap();
            if old {
                let file = fs::File::options().write(true).open(&path).unwrap();
                file.set_modified(old_time).unwrap();
            }
            path
        };
        let stale = make("stone-index-1-1-0", true);
        let stale_lock = make("stone-index-1-1-0.lock", true);
        let fresh = make("stone-index-2-2-0", false);
        let unrelated = make("not-ours", true);
        clean_stale(&repo);
        assert!(!stale.exists() && !stale_lock.exists());
        assert!(fresh.exists() && unrelated.exists());
    }
}
