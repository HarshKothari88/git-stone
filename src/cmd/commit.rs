//! Turn the first stone into a normal Git commit whose tree equals the stone's tree (A8, I5).
//!
//! The commit is made by a real `git commit` against a temporary index that holds exactly the stone.
//! That keeps hooks, signing, identity, templates and the editor working with no code of ours.
//! The user's real index is then moved forward with `read-tree -m`, which keeps their staged work.

use std::fs;

use super::{
    find_stone, guard, load_state, require_clean_start, short, sync_chain, CmdError, CmdResult,
};
use crate::failpoint::failpoint;
use crate::git::Git;
use crate::repo::Repo;
use crate::snapshot::{self, TempIndex};
use crate::state::StateError;

const ALLOWED: &str = "-m/--message, -F/--file, -e/--edit, --no-verify, -S/--gpg-sign, --author, --date, -s/--signoff";

/// Only options we know are safe with a temporary index are passed on to `git commit`.
fn check_args(args: &[String]) -> Result<Vec<String>, CmdError> {
    let mut checked = Vec::new();
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        let name = arg.as_str();
        match name {
            "-e" | "--edit" | "--no-verify" | "-n" | "-S" | "--gpg-sign" | "-s" | "--signoff" => {
                checked.push(arg.clone())
            }
            "-m" | "--message" | "-F" | "--file" | "--author" | "--date" => {
                let Some(value) = rest.next() else {
                    return Err(CmdError::user(format!("`{name}` needs a value.")));
                };
                checked.push(arg.clone());
                checked.push(value.clone());
            }
            _ if [
                "--message=",
                "--file=",
                "--author=",
                "--date=",
                "--gpg-sign=",
            ]
            .iter()
            .any(|prefix| name.starts_with(prefix)) =>
            {
                checked.push(arg.clone())
            }
            // `-S<keyid>`
            _ if name.starts_with("-S") && !name.starts_with("--") => checked.push(arg.clone()),
            _ => {
                return Err(CmdError::user(format!(
                    "`git stone commit` does not support `{name}` yet. Supported: {ALLOWED}."
                )))
            }
        }
    }
    Ok(checked)
}

/// `git stone <id> commit [options]`
pub fn run(repo: &Repo, id: u64, args: &[String]) -> CmdResult {
    let commit_args = check_args(args)?;
    snapshot::clean_stale(repo);
    require_clean_start(repo, "committing")?;

    let Some((oid, state)) = load_state(repo)? else {
        find_stone(None, id)?;
        unreachable!("find_stone fails without a state");
    };
    // Stones that are already committed on the branch are marked so first; this also heals a commit that was interrupted.
    let (oid, mut state) = sync_chain(repo, oid, state)?;
    let (index, stone) = find_stone(Some(&state), id)?;
    if index != 0 {
        return Err(CmdError::user(format!(
            "Commit Stone {first} first, or drop it to fold it into Stone {id}.",
            first = state.stones[0].id
        )));
    }
    let stone = stone.clone();

    // The commit would be made first and the index update would fail after it, so look now.
    let lock = format!("{}.lock", repo.index_path.display());
    if fs::metadata(&lock).is_ok() {
        return Err(CmdError::user(format!(
            "The staging area is locked ({lock}). Another git process may be running; if not, remove that file and try again."
        )));
    }

    // H0 is the commit the stone sits on; for the first commit on an unborn branch, the empty tree.
    let h0 = repo.head.clone().unwrap_or_else(|| repo.empty_tree.clone());

    // Dry run of the index update on a copy: if the user's staged work would clash, find out before anything is committed.
    {
        let copy = TempIndex::create(repo)?;
        let dry = repo
            .git()
            .with_index(copy.path())
            .allow_optional_locks()
            .run(["read-tree", "-m", "-i", &h0, &stone.tree]);
        if let Err(e) = dry {
            return Err(CmdError::user(format!(
                "Your staged changes clash with Stone {id}, so nothing was committed.\nUnstage the clashing paths (`git restore --staged <path>`) and try again.\n{}",
                e.stderr.trim_end()
            )));
        }
    }

    // The commit itself, on a temporary index that holds exactly the stone.
    {
        let temp = TempIndex::create(repo)?;
        // A copy of the real index moved onto the stone's tree. A single-tree merge makes the index equal to the
        // tree (staged work that is not in the stone is gone from this copy) but keeps the file stat data of entries
        // that already match. A plain `read-tree <tree>` drops all stat data, and `git commit` then re-hashes every
        // file: 4.4 s instead of 0.1 s at 50k files (Phase 8 finding). `-i`: never look at the working tree.
        let temp_git = repo.git().with_index(temp.path());
        temp_git.run(["read-tree", "-m", "-i", &stone.tree])?;
        // The commit must hold exactly the stone (I5). Refuse before committing if the index is anything else.
        let held = temp_git.run(["write-tree"])?;
        if String::from_utf8_lossy(&held).trim() != stone.tree {
            return Err(CmdError {
                code: 2,
                message: format!(
                    "internal error: the index prepared for Stone {id} does not equal the stone. Nothing was committed."
                ),
            });
        }
        // Starts in the user's own directory (a relative `-F file` means what they think), on their terminal, with locks allowed.
        let mut args = vec!["commit".to_string()];
        args.extend(commit_args);
        let status = Git::new()
            .with_index(temp.path())
            // Tells the guard hook (A17) that this commit is the stone's own.
            .with_env(guard::ENV, "1")
            .allow_optional_locks()
            .run_inherit(args)?;
        if !status.success() {
            return Err(CmdError {
                code: status.code().unwrap_or(2),
                message: "git commit did not make a commit. Nothing was changed.".to_string(),
            });
        }
    }
    failpoint("commit:after-git-commit");

    // The branch must have moved by exactly our commit.
    let git = repo.git();
    let line = git.run(["rev-list", "--parents", "-n1", "HEAD"])?;
    let line = String::from_utf8_lossy(&line).trim().to_string();
    let mut ids = line.split(' ');
    let new_head = ids.next().unwrap_or_default().to_string();
    let parents: Vec<&str> = ids.collect();
    let expected: Vec<&str> = repo.head.as_deref().into_iter().collect();
    if parents != expected {
        return Err(CmdError::user(
            "The commit was made, but the branch also moved for another reason at the same time.\nThe stones and your staged changes were left as they were. Run `git stone status` to see where things stand.",
        ));
    }

    // Move the user's index forward. Safe to repeat, so a crash before this line is healed by the next command.
    // `-i`: never look at the working tree. Entries made by `read-tree` carry no file stamps, so without it
    // a later merge says "not uptodate" for a file the user has edited since (VERIFY finding, docs/design.md).
    if let Err(e) = git
        .clone()
        .allow_optional_locks()
        .run(["read-tree", "-m", "-i", &h0, "HEAD"])
    {
        return Err(CmdError {
            code: 2,
            message: format!(
                "The commit {} was made, but your staged changes could not be moved forward:\n{}\nTo finish, run: git read-tree -m -i {h0} HEAD\nThe next git stone command then records the commit.",
                short(&new_head),
                e.stderr.trim_end()
            ),
        });
    }
    failpoint("commit:after-index-update");

    // A hook (a formatter that re-adds files, for example) can change what was committed.
    let head_tree = git.run(["rev-parse", "HEAD^{tree}"])?;
    let head_tree = String::from_utf8_lossy(&head_tree).trim().to_string();
    if head_tree != stone.tree {
        let changed = git.run([
            "diff-tree",
            "-r",
            "-z",
            "--name-only",
            &stone.tree,
            &head_tree,
        ])?;
        let paths: Vec<String> = changed
            .split(|b| *b == 0)
            .filter(|p| !p.is_empty())
            .map(|p| format!("  {}", String::from_utf8_lossy(p)))
            .collect();
        eprintln!(
            "Warning: the commit is not exactly Stone {id}; a hook changed these paths:\n{}\nThe next stone will start from the commit as it was made.",
            paths.join("\n")
        );
    }

    // Record it: the stone is gone, the commit is the new anchor.
    state.stones.remove(0);
    state.anchor = Some(new_head.clone());
    match state.save(repo, Some(&oid), &format!("commit {id}")) {
        Ok(_) => {}
        Err(StateError::Conflict) => {
            return Err(CmdError::user(
                "The commit was made, but another git stone command changed the stones at the same time.\nThe next git stone command records the commit.",
            ))
        }
        Err(e) => return Err(e.into()),
    }

    let subject = git.run(["log", "-1", "--format=%s", "HEAD"])?;
    println!(
        "Committed Stone {id} as {}: {}",
        short(&new_head),
        String::from_utf8_lossy(&subject).trim_end()
    );
    match state.stones.first() {
        Some(next) => println!("Stone {} is now next.", next.id),
        None => println!("No stones are left."),
    }
    Ok(0)
}

/// `git stone commit-all [--edit]`: every stone in order, the stone's name as the message.
/// Stops at the first failure; the commits already made stay made and recorded.
pub fn run_all(repo: &Repo, edit: bool) -> CmdResult {
    let mut repo = Repo::discover_in(&repo.top)?;
    let mut made = 0;
    while let Some((oid, state)) = load_state(&repo)? {
        let (_, state) = sync_chain(&repo, oid, state)?;
        let Some(first) = state.stones.first() else {
            break;
        };
        let mut args = vec!["-m".to_string(), first.name.clone()];
        if edit {
            args.push("-e".to_string());
        }
        run(&repo, first.id, &args)?;
        made += 1;
        // HEAD moved, so look at the repository again.
        repo = Repo::discover_in(&repo.top)?;
    }
    if made == 0 {
        return Err(CmdError::user("There are no stones to commit."));
    }
    Ok(0)
}
