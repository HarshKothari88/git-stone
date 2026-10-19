//! Paths left out of stones: the `stone.exclude` entries of this repository's own git config.
//!
//! The rules are git pathspecs, so `*.log` and `scratch/` both work. They live in `.git/config`, which
//! every branch and worktree of this clone shares. They are never pushed and never cloned.

use super::{CmdError, CmdResult};
use crate::git::Git;
use crate::repo::Repo;
use crate::ExcludeAction;

const KEY: &str = "stone.exclude";

/// Every saved rule.
pub fn read(repo: &Repo) -> Result<Vec<String>, CmdError> {
    // Exit 1 means the key is not set.
    let (_, out) = repo
        .git()
        .run_answers(["config", "--local", "-z", "--get-all", KEY], &[1])?;
    Ok(out
        .split(|b| *b == 0)
        .filter(|v| !v.is_empty())
        .map(|v| String::from_utf8_lossy(v).into_owned())
        .collect())
}

/// Rules as typed in the current folder, checked, and made relative to the top of the repository.
pub fn normalize(patterns: &[String]) -> Result<Vec<String>, CmdError> {
    let prefix = Git::new().run(["rev-parse", "--show-prefix"])?;
    let prefix = String::from_utf8_lossy(&prefix).trim_end().to_string();
    patterns
        .iter()
        .map(|p| {
            if p.is_empty() || p.chars().any(char::is_control) {
                return Err(CmdError::user(
                    "An exclude pattern must not be empty or hold control characters.",
                ));
            }
            // Like .gitignore: a leading `/` means "from the top of the repository".
            let full = match p.strip_prefix('/') {
                Some(rooted) => rooted.to_string(),
                None => format!("{prefix}{p}"),
            };
            if full.is_empty() || full.split('/').any(|part| part == "..") {
                return Err(CmdError::user(format!(
                    "`{p}` must point inside the repository (no `..`)."
                )));
            }
            Ok(full)
        })
        .collect()
}

/// Save rules that are not saved yet.
pub fn add(repo: &Repo, patterns: &[String]) -> Result<(), CmdError> {
    let have = read(repo)?;
    for p in patterns.iter().filter(|p| !have.contains(p)) {
        repo.git()
            .run(["config", "--local", "--add", "--", KEY, p])?;
    }
    Ok(())
}

/// How many files a rule matches right now (tracked, or untracked and not ignored).
pub fn matches(repo: &Repo, pattern: &str) -> Result<usize, CmdError> {
    let out = repo.git().run([
        "ls-files",
        "-z",
        "-c",
        "-o",
        "--exclude-standard",
        "--",
        &format!(":(top){pattern}"),
    ])?;
    Ok(out.iter().filter(|b| **b == 0).count())
}

/// `Left out: *.log (3 files), scratch/ (12 files)`. A rule that shows `(0 files)` is often a typo.
pub fn describe(repo: &Repo, rules: &[String]) -> Result<String, CmdError> {
    let mut parts = Vec::new();
    for rule in rules {
        let n = matches(repo, rule)?;
        let unit = if n == 1 { "file" } else { "files" };
        parts.push(format!("{rule} ({n} {unit})"));
    }
    Ok(parts.join(", "))
}

/// `git stone exclude [--remove <pattern> | --clear]`
pub fn run(repo: &Repo, action: &ExcludeAction) -> CmdResult {
    match action {
        ExcludeAction::List => {
            let rules = read(repo)?;
            if rules.is_empty() {
                println!(
                    "No exclude rules. Add one with: git stone \"name\" -x <pattern> --remember"
                );
            } else {
                println!("Left out of every stone in this repository, on every branch:");
                for rule in &rules {
                    println!("  {rule}");
                }
            }
        }
        ExcludeAction::Remove(pattern) => {
            let [full] = &normalize(std::slice::from_ref(pattern))?[..] else {
                unreachable!("one pattern in, one out");
            };
            // Exit 5 means no entry has this value.
            let (code, _) = repo.git().run_answers(
                ["config", "--local", "--fixed-value", "--unset", KEY, full],
                &[5],
            )?;
            if code == 5 {
                return Err(CmdError::user(format!(
                    "`{full}` is not an exclude rule. See: git stone exclude"
                )));
            }
            println!("Removed `{full}`. The next stone includes matching files again.");
        }
        ExcludeAction::Clear => {
            repo.git()
                .run_answers(["config", "--local", "--unset-all", KEY], &[5])?;
            println!("No exclude rules left. The next stone includes everything again.");
        }
    }
    Ok(0)
}
