//! Runs the `git` binary. Argv lists only, never a shell string (I8).

use std::ffi::{OsStr, OsString};
use std::fmt;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Output, Stdio};
use std::thread;

/// A git call that failed. `code` is `None` when git could not be started at all.
#[derive(Debug)]
pub struct GitError {
    pub args: Vec<String>,
    pub code: Option<i32>,
    pub stderr: String,
}

pub type Result<T> = std::result::Result<T, GitError>;

impl fmt::Display for GitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let args = self.args.join(" ");
        match self.code {
            Some(code) => write!(f, "git {args} failed (exit {code})")?,
            None => write!(f, "git {args} could not be started")?,
        }
        let stderr = self.stderr.trim_end();
        if !stderr.is_empty() {
            write!(f, ":\n{stderr}")?;
        }
        Ok(())
    }
}

impl std::error::Error for GitError {}

/// Settings for the git calls that follow. Build one, then call `run*` on it as often as needed.
#[derive(Clone, Default)]
pub struct Git {
    dir: Option<PathBuf>,
    env: Vec<(OsString, OsString)>,
    optional_locks: bool,
}

impl Git {
    pub fn new() -> Git {
        Git::default()
    }

    pub fn in_dir(mut self, dir: impl Into<PathBuf>) -> Git {
        self.dir = Some(dir.into());
        self
    }

    pub fn with_env(mut self, key: impl Into<OsString>, val: impl Into<OsString>) -> Git {
        self.env.push((key.into(), val.into()));
        self
    }

    /// Point git at another index file (`GIT_INDEX_FILE`).
    pub fn with_index(self, path: &Path) -> Git {
        self.with_env("GIT_INDEX_FILE", path)
    }

    /// By default `GIT_OPTIONAL_LOCKS=0`, so a read never refreshes the real index behind the user's back.
    pub fn allow_optional_locks(mut self) -> Git {
        self.optional_locks = true;
        self
    }

    /// Run git, return its stdout. Any non-zero exit is an error carrying git's stderr.
    pub fn run<I, S>(&self, args: I) -> Result<Vec<u8>>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let args = collect(args);
        let out = self
            .command(&args)
            .output()
            .map_err(|e| spawn_error(&args, e))?;
        finish(&args, out)
    }

    /// Like `run`, but some non-zero exits are answers, not failures (`merge-tree` exits 1 on a conflict).
    /// Gives the exit code (0 or one of `answers`) with stdout; any other exit is an error.
    pub fn run_answers<I, S>(&self, args: I, answers: &[i32]) -> Result<(i32, Vec<u8>)>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let args = collect(args);
        let out = self
            .command(&args)
            .output()
            .map_err(|e| spawn_error(&args, e))?;
        match out.status.code() {
            Some(code) if code == 0 || answers.contains(&code) => Ok((code, out.stdout)),
            _ => finish(&args, out).map(|stdout| (0, stdout)),
        }
    }

    /// Like `run`, with `input` written to git's stdin.
    pub fn run_stdin<I, S>(&self, args: I, input: &[u8]) -> Result<Vec<u8>>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let args = collect(args);
        let mut child = self
            .command(&args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| spawn_error(&args, e))?;
        let mut stdin = child.stdin.take().expect("stdin was piped");
        // Write on a side thread so a big input and a big output cannot block each other.
        // A failed write (git exited early) is reported through git's exit status instead.
        let out = thread::scope(|s| {
            s.spawn(move || {
                let _ = stdin.write_all(input);
            });
            child.wait_with_output()
        })
        .map_err(|e| spawn_error(&args, e))?;
        finish(&args, out)
    }

    /// Run git on the user's terminal (stdin, stdout, stderr inherited). The caller decides what the exit status means.
    pub fn run_inherit<I, S>(&self, args: I) -> Result<ExitStatus>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let args = collect(args);
        self.command(&args)
            .status()
            .map_err(|e| spawn_error(&args, e))
    }

    fn command(&self, args: &[OsString]) -> Command {
        let mut cmd = Command::new("git");
        cmd.args(args);
        if let Some(dir) = &self.dir {
            cmd.current_dir(dir);
        }
        if !self.optional_locks {
            cmd.env("GIT_OPTIONAL_LOCKS", "0");
        }
        for (key, val) in &self.env {
            cmd.env(key, val);
        }
        cmd
    }
}

fn collect<I, S>(args: I) -> Vec<OsString>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    args.into_iter()
        .map(|a| a.as_ref().to_os_string())
        .collect()
}

fn error(args: &[OsString], code: Option<i32>, stderr: String) -> GitError {
    GitError {
        args: args
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect(),
        code,
        stderr,
    }
}

fn spawn_error(args: &[OsString], e: io::Error) -> GitError {
    error(args, None, e.to_string())
}

fn finish(args: &[OsString], out: Output) -> Result<Vec<u8>> {
    if out.status.success() {
        Ok(out.stdout)
    } else {
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        Err(error(args, out.status.code(), stderr))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn elsewhere() -> Git {
        Git::new().in_dir(std::env::temp_dir())
    }

    #[test]
    fn run_returns_stdout() {
        let out = elsewhere().run(["--version"]).unwrap();
        assert!(out.starts_with(b"git version"));
    }

    #[test]
    fn failure_carries_args_code_and_stderr() {
        let err = elsewhere().run(["frobnicate"]).unwrap_err();
        assert_eq!(err.args, ["frobnicate"]);
        assert_eq!(err.code, Some(1));
        assert!(err.stderr.contains("frobnicate"));
        assert!(err.to_string().contains("git frobnicate failed (exit 1)"));
    }

    #[test]
    fn run_stdin_feeds_input() {
        // Well-known object id of the blob "hello\n" in a SHA-1 repo.
        let out = elsewhere()
            .with_env("GIT_DEFAULT_HASH", "sha1")
            .run_stdin(["hash-object", "--stdin"], b"hello\n")
            .unwrap();
        assert_eq!(out, b"ce013625030ba8dba906f756967f9e9ca394464a\n");
    }

    #[test]
    fn run_stdin_handles_large_input() {
        // Bigger than a pipe buffer, so a naive write-then-read would hang.
        let input = vec![b'x'; 4 * 1024 * 1024];
        let out = elsewhere()
            .with_env("GIT_DEFAULT_HASH", "sha1")
            .run_stdin(["hash-object", "--stdin"], &input)
            .unwrap();
        assert_eq!(out.len(), 41);
    }

    #[test]
    fn run_inherit_reports_status_without_error() {
        let ok = elsewhere().run_inherit(["--version"]).unwrap();
        assert!(ok.success());
        let bad = elsewhere().run_inherit(["frobnicate"]).unwrap();
        assert_eq!(bad.code(), Some(1));
    }

    #[test]
    fn optional_locks_off_by_default() {
        // A shell alias lets git print the environment it gave its children.
        let alias = ["-c", "alias.e=!printf %s \"$GIT_OPTIONAL_LOCKS\"", "e"];
        assert_eq!(elsewhere().run(alias).unwrap(), b"0");
        let allowed = elsewhere().allow_optional_locks().run(alias).unwrap();
        assert_eq!(allowed, b"");
    }

    #[test]
    fn with_index_sets_git_index_file() {
        let alias = ["-c", "alias.e=!printf %s \"$GIT_INDEX_FILE\"", "e"];
        let out = elsewhere()
            .with_index(Path::new("/some/index"))
            .run(alias)
            .unwrap();
        assert_eq!(out, b"/some/index");
    }
}
