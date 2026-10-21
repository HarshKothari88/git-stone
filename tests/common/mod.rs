//! A temp repo, a way to run the real `git-stone` binary in it, and the "nothing else changed" check (I1).
#![allow(dead_code)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use tempfile::TempDir;

pub struct Out {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

fn out(o: Output) -> Out {
    Out {
        code: o.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&o.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&o.stderr).into_owned(),
    }
}

/// No user or system config, fixed identity, and HOME inside the temp dir.
fn hermetic(cmd: &mut Command, home: &Path) {
    cmd.env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_AUTHOR_DATE", "2026-01-01T00:00:00Z")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_DATE", "2026-01-01T00:00:00Z")
        .env("HOME", home)
        // The guard hook calls `git-stone`, so the binary under test must be on PATH.
        .env("PATH", path_with_binary())
        .env_remove("GIT_STONE_FAILPOINT")
        .env_remove("GIT_INDEX_FILE")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE");
}

fn path_with_binary() -> std::ffi::OsString {
    let bin = Path::new(env!("CARGO_BIN_EXE_git-stone"))
        .parent()
        .expect("the binary has a folder")
        .to_path_buf();
    let mut paths = vec![bin];
    paths.extend(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    ));
    std::env::join_paths(paths).expect("PATH entries are valid")
}

pub struct Repo {
    /// Keeps the directory alive.
    pub tmp: TempDir,
    /// The working tree.
    pub dir: PathBuf,
}

/// What must not change when a command only reads or only records a stone.
#[derive(Debug, PartialEq, Eq)]
pub struct Fingerprint {
    head_file: Vec<u8>,
    branch_oid: String,
    index: Option<Vec<u8>>,
    files: Vec<(String, u32, Vec<u8>)>,
}

impl Repo {
    /// A repo with one commit (`base.txt`) on `main`.
    pub fn new() -> Repo {
        let repo = Repo::unborn();
        repo.write("base.txt", "base\n");
        repo.git(&["add", "base.txt"]);
        repo.git(&["commit", "-q", "-m", "base"]);
        repo
    }

    /// A repo with no commit yet.
    pub fn unborn() -> Repo {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().canonicalize().unwrap().join("work");
        fs::create_dir(&dir).unwrap();
        let repo = Repo { tmp, dir };
        repo.git(&["init", "-q", "-b", "main"]);
        repo
    }

    pub fn home(&self) -> PathBuf {
        self.tmp.path().to_path_buf()
    }

    pub fn git(&self, args: &[&str]) -> String {
        self.git_in(&self.dir, args)
    }

    pub fn git_in(&self, dir: &Path, args: &[&str]) -> String {
        let o = self.git_try_in(dir, args);
        assert_eq!(o.code, 0, "git {args:?} failed: {}", o.stderr);
        o.stdout
    }

    pub fn git_try(&self, args: &[&str]) -> Out {
        self.git_try_in(&self.dir, args)
    }

    pub fn git_try_in(&self, dir: &Path, args: &[&str]) -> Out {
        let mut cmd = Command::new("git");
        cmd.args(args).current_dir(dir);
        hermetic(&mut cmd, &self.home());
        out(cmd.output().unwrap())
    }

    /// Run `git-stone` in the working tree.
    pub fn stone(&self, args: &[&str]) -> Out {
        self.stone_in(&self.dir, args)
    }

    pub fn stone_in(&self, dir: &Path, args: &[&str]) -> Out {
        self.stone_env(dir, args, &[])
    }

    pub fn stone_env(&self, dir: &Path, args: &[&str], env: &[(&str, &str)]) -> Out {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_git-stone"));
        cmd.args(args).current_dir(dir);
        hermetic(&mut cmd, &self.home());
        for (k, v) in env {
            cmd.env(k, v);
        }
        out(cmd.output().unwrap())
    }

    /// `stone` that must succeed.
    pub fn ok(&self, args: &[&str]) -> String {
        let o = self.stone(args);
        assert_eq!(
            o.code, 0,
            "git-stone {args:?} failed: {}{}",
            o.stdout, o.stderr
        );
        o.stdout
    }

    pub fn write(&self, path: &str, text: &str) {
        let full = self.dir.join(path);
        fs::create_dir_all(full.parent().unwrap()).unwrap();
        fs::write(full, text).unwrap();
    }

    pub fn read(&self, path: &str) -> String {
        fs::read_to_string(self.dir.join(path)).unwrap()
    }

    /// The snapshot tree of stone `id` on `branch`.
    pub fn stone_tree(&self, branch: &str, id: u64) -> String {
        self.git(&[
            "rev-parse",
            &format!("refs/stones/heads/{branch}:trees/{id}"),
        ])
        .trim()
        .to_string()
    }

    pub fn state_ref_exists(&self, name: &str) -> bool {
        self.git_try(&["rev-parse", "-q", "--verify", name]).code == 0
    }

    /// Every file in the working tree (not `.git`): path, mode, bytes.
    pub fn files(&self) -> Vec<(String, u32, Vec<u8>)> {
        let mut files = Vec::new();
        walk(&self.dir, &self.dir, &mut files);
        files.sort();
        files
    }

    /// A Git hook; `body` is a POSIX shell script without the first line.
    pub fn hook(&self, name: &str, body: &str) {
        let path = self.dir.join(".git/hooks").join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    }

    /// Anything the stones left in the git dir that should have been cleaned up.
    pub fn temp_indexes(&self) -> Vec<String> {
        fs::read_dir(self.dir.join(".git"))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with("stone-index-"))
            .collect()
    }

    pub fn fingerprint(&self) -> Fingerprint {
        let git_dir = self.dir.join(".git");
        let branch_oid = self
            .git_try(&["rev-parse", "-q", "--verify", "HEAD"])
            .stdout;
        let mut files = Vec::new();
        walk(&self.dir, &self.dir, &mut files);
        files.sort();
        Fingerprint {
            head_file: fs::read(git_dir.join("HEAD")).unwrap(),
            branch_oid,
            index: fs::read(git_dir.join("index")).ok(),
            files,
        }
    }

    /// I1: the branch, the real index and every file (also ignored and untracked) are as they were.
    pub fn assert_untouched(&self, before: &Fingerprint) {
        assert_eq!(&self.fingerprint(), before);
    }
}

fn walk(root: &Path, dir: &Path, files: &mut Vec<(String, u32, Vec<u8>)>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.file_name().is_some_and(|n| n == ".git") {
            continue;
        }
        let meta = fs::symlink_metadata(&path).unwrap();
        let rel = path
            .strip_prefix(root)
            .unwrap()
            .to_string_lossy()
            .into_owned();
        if meta.is_dir() {
            walk(root, &path, files);
        } else if meta.file_type().is_symlink() {
            let target = fs::read_link(&path).unwrap();
            files.push((
                rel,
                meta.permissions().mode(),
                target.to_string_lossy().into_owned().into_bytes(),
            ));
        } else {
            files.push((rel, meta.permissions().mode(), fs::read(&path).unwrap()));
        }
    }
}
