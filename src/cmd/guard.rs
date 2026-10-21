//! The guard: while a branch has stones, a plain git command may not move that branch (A17).
//!
//! A tiny `reference-transaction` hook asks `git-stone guard-check` before every branch update. Git runs that hook
//! also for `git commit --no-verify`, and an exit code of 1 in its `prepared` phase cancels the update. The `git commit`
//! that `git stone commit` runs itself carries `GIT_STONE_COMMIT`, so it is let through.
//!
//! The setting is `stone.guard` (`block` or `warn`) in this clone's own git config, like the exclude rules.

use std::fs;
use std::io::Read;
use std::path::PathBuf;

use super::{CmdError, CmdResult};
use crate::git::Git;
use crate::repo::Repo;
use crate::state::State;
use crate::GuardAction;

const KEY: &str = "stone.guard";
/// Set on the `git commit` that Git Stone runs itself, so the hook lets it through.
pub const ENV: &str = "GIT_STONE_COMMIT";
/// What `guard-check` answers to mean "block". The hook turns it into exit 1. Any other answer allows, so an older
/// git-stone that does not know the command (it exits 1) can never block a commit.
const BLOCK_CODE: i32 = 3;
const HOOK_MARK: &str = "# git-stone guard";
const HOOK: &str = "#!/bin/sh
# git-stone guard (managed by `git stone guard`; turn it off with `git stone guard off`)
command -v git-stone >/dev/null 2>&1 || exit 0
git-stone guard-check \"$@\"
[ $? -eq 3 ] && exit 1
exit 0
";
/// The same lines, to add by hand to a hook Git Stone does not own.
const SNIPPET: &str = "  command -v git-stone >/dev/null 2>&1 && {
    git-stone guard-check \"$@\"
    [ $? -eq 3 ] && exit 1
  }";

/// Where the hook is, and who owns it.
enum Hook {
    Ours(PathBuf),
    /// A hook of someone else is there: never touched.
    Foreign(PathBuf),
    /// Nothing there yet, and the hooks folder is the normal one.
    Missing(PathBuf),
    /// `core.hooksPath` points elsewhere (husky and similar, often tracked): never written to.
    Custom(PathBuf),
}

/// `git stone guard [block | warn | off]`
pub fn run(repo: &Repo, action: &GuardAction) -> CmdResult {
    match action {
        GuardAction::Show => show(repo),
        GuardAction::Block => set(repo, "block"),
        GuardAction::Warn => set(repo, "warn"),
        GuardAction::Off => off(repo),
    }
}

fn mode(git: &Git) -> Result<Option<String>, CmdError> {
    let (code, out) = git.run_answers(["config", "--get", KEY], &[1])?;
    let value = String::from_utf8_lossy(&out).trim().to_string();
    Ok((code == 0 && (value == "block" || value == "warn")).then_some(value))
}

fn hook_state(repo: &Repo) -> Result<Hook, CmdError> {
    let out = repo
        .git()
        .run(["rev-parse", "--path-format=absolute", "--git-path", "hooks"])?;
    let path =
        PathBuf::from(String::from_utf8_lossy(&out).trim_end()).join("reference-transaction");
    match fs::read_to_string(&path) {
        Ok(text) if text.contains(HOOK_MARK) => Ok(Hook::Ours(path)),
        Ok(_) => Ok(Hook::Foreign(path)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let (code, _) = repo
                .git()
                .run_answers(["config", "--get", "core.hooksPath"], &[1])?;
            Ok(if code == 0 {
                Hook::Custom(path)
            } else {
                Hook::Missing(path)
            })
        }
        Err(e) => Err(CmdError::user(format!(
            "Cannot read {}: {e}",
            path.display()
        ))),
    }
}

fn write_hook(path: &PathBuf) -> Result<(), CmdError> {
    let fail = |e: std::io::Error| CmdError {
        code: 2,
        message: format!("cannot write {}: {e}", path.display()),
    };
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(fail)?;
    }
    fs::write(path, HOOK).map_err(fail)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).map_err(fail)?;
    }
    Ok(())
}

fn set(repo: &Repo, value: &str) -> CmdResult {
    // The hook first, so a failure leaves no setting that promises something that is not there.
    let active = match hook_state(repo)? {
        Hook::Missing(path) => {
            write_hook(&path)?;
            Ok(format!("Hook installed: {}", path.display()))
        }
        Hook::Ours(path) => {
            if fs::read_to_string(&path).is_ok_and(|t| t != HOOK) {
                write_hook(&path)?;
            }
            Ok(format!("Hook installed: {}", path.display()))
        }
        Hook::Foreign(path) => Err(format!(
            "{} is a hook of your own, so Git Stone left it alone.",
            path.display()
        )),
        Hook::Custom(path) => Err(format!(
            "core.hooksPath points elsewhere, so Git Stone did not write {}.",
            path.display()
        )),
    };
    repo.git()
        .run(["config", "--local", "--replace-all", "--", KEY, value])?;
    println!("Guard is now `{value}` for this repository.");
    match active {
        Ok(line) => {
            println!("{line}");
            if value == "block" {
                println!("While a branch has stones, a plain git command that would move it is refused. `git stone <id> commit` still works.");
            } else {
                println!("While a branch has stones, a plain git command that would move it prints a warning.");
            }
        }
        Err(why) => {
            println!("The setting is saved but the guard is NOT active yet. {why}");
            println!("To activate it, put these lines in that hook (before it reads its input, if it does):\n{SNIPPET}");
        }
    }
    Ok(0)
}

fn off(repo: &Repo) -> CmdResult {
    repo.git()
        .run_answers(["config", "--local", "--unset-all", KEY], &[5])?;
    match hook_state(repo)? {
        Hook::Ours(path) => {
            fs::remove_file(&path).map_err(|e| CmdError {
                code: 2,
                message: format!("cannot remove {}: {e}", path.display()),
            })?;
            println!("Guard is off. Hook removed: {}", path.display());
        }
        Hook::Foreign(path) => println!(
            "Guard is off. {} is not ours, so it stays; if you added the guard lines to it by hand, remove them.",
            path.display()
        ),
        _ => println!("Guard is off."),
    }
    Ok(0)
}

fn show(repo: &Repo) -> CmdResult {
    let value = mode(&repo.git())?;
    println!("Guard: {}", value.as_deref().unwrap_or("off"));
    match hook_state(repo)? {
        Hook::Ours(path) => println!("Hook: installed ({})", path.display()),
        Hook::Foreign(path) => println!("Hook: {} exists and is not ours", path.display()),
        Hook::Missing(_) if value.is_some() => println!("Hook: missing. Run `git stone guard {}` again.", value.as_deref().unwrap_or("block")),
        Hook::Custom(_) if value.is_some() => println!("Hook: core.hooksPath is set; add the guard lines to its reference-transaction hook:\n{SNIPPET}"),
        _ => println!("Hook: not installed"),
    }
    if value.is_none() {
        println!("Turn it on with: git stone guard block");
    }
    Ok(0)
}

/// `git stone guard-check <phase>`, called by the hook with the ref updates on stdin. Returns 3 to block.
/// Every error and every doubt allows: the guard must never wedge a repository.
pub fn check(phase: &str) -> i32 {
    // Git feeds the hook on stdin; read it all, also when the answer is already clear.
    let mut input = Vec::new();
    let _ = std::io::stdin().read_to_end(&mut input);
    decide(phase, &String::from_utf8_lossy(&input)).unwrap_or(0)
}

fn decide(phase: &str, input: &str) -> Result<i32, CmdError> {
    if phase != "prepared" || std::env::var_os(ENV).is_some_and(|v| !v.is_empty()) {
        return Ok(0);
    }
    let Some(value) = mode(&Git::new())? else {
        return Ok(0);
    };
    // `<old> <new> <ref>`. Only real branch moves count: not creations, deletions, no-op updates or stone refs.
    let branches: Vec<&str> = input
        .lines()
        .filter_map(|line| {
            let mut parts = line.splitn(3, ' ');
            let (old, new, name) = (parts.next()?, parts.next()?, parts.next()?);
            let zero = |oid: &str| oid.bytes().all(|b| b == b'0');
            (name.starts_with("refs/heads/") && !zero(old) && !zero(new) && old != new)
                .then_some(name)
        })
        .collect();
    if branches.is_empty() {
        return Ok(0);
    }
    let repo = Repo::discover()?;
    for branch in branches {
        let state_ref = format!(
            "refs/stones/{}",
            branch.strip_prefix("refs/").unwrap_or(branch)
        );
        let Some((_, state)) = State::load_ref(&repo, &state_ref, branch)? else {
            continue;
        };
        if state.stones.is_empty() {
            continue;
        }
        let name = branch.strip_prefix("refs/heads/").unwrap_or(branch);
        let count = state.stones.len();
        let noun = if count == 1 { "stone" } else { "stones" };
        if value == "block" {
            eprintln!(
                "Git Stone guard: branch `{name}` has {count} {noun}, so a plain git command may not move it.\nCommit the stones with `git stone <id> commit` or `git stone commit-all`, or remove them with `git stone drop <id>` or `git stone drop all`.\nNothing was changed. Ask the human before turning the guard off."
            );
            return Ok(BLOCK_CODE);
        }
        eprintln!(
            "Warning: branch `{name}` has {count} {noun} and a plain git command moved it. The stones may need `git stone repair`.\nUse `git stone <id> commit` or `git stone commit-all` to commit stones."
        );
    }
    Ok(0)
}
