use std::collections::HashSet;
use std::fs;

use super::{printable, CmdError, CmdResult};
use crate::repo::{encode_worktree_name, Repo};
use crate::snapshot;
use crate::state::State;

const HEADS: &str = "refs/stones/heads/";
const DETACHED: &str = "refs/stones/detached/";

/// A chain whose branch or worktree is gone.
struct Leftover {
    /// The full state ref, e.g. `refs/stones/heads/old`.
    reference: String,
    /// What the user knows it as.
    label: String,
    /// The state commit the ref holds now.
    oid: String,
    /// Names of the stones it still holds.
    stones: Vec<String>,
    /// Why it cannot be read, if so. Never removed without being named.
    unreadable: Option<String>,
}

impl Leftover {
    fn empty(&self) -> bool {
        self.stones.is_empty() && self.unreadable.is_none()
    }
}

/// List (and with `--apply` remove) chains that nothing can reach any more, and stale temp files (A12).
/// Chains that still hold stones are only removed when named with `--chain`.
pub fn run(repo: &Repo, apply: bool, chain: Option<&str>) -> CmdResult {
    let found = leftovers(repo)?;
    let (removable, kept): (Vec<&Leftover>, Vec<&Leftover>) = match chain {
        None => found.iter().partition(|l| l.empty()),
        Some(target) => {
            if !target.starts_with(HEADS) && !target.starts_with(DETACHED) {
                return Err(CmdError::user(format!(
                    "`{target}` is not a stone chain. Chains are named like {HEADS}<branch>; `git stone prune` lists them."
                )));
            }
            let Some(item) = found.iter().find(|l| l.reference == target) else {
                let in_use = repo
                    .git()
                    .run(["rev-parse", "-q", "--verify", target])
                    .is_ok();
                return Err(CmdError::user(if in_use {
                    format!("`{target}` still belongs to a branch or worktree that exists, so it is not a leftover.")
                } else {
                    format!("There is no chain `{target}`. `git stone prune` lists the leftovers.")
                }));
            };
            (vec![item], Vec::new())
        }
    };
    let temp = if chain.is_none() {
        snapshot::stale_indexes(repo)
    } else {
        Vec::new()
    };

    if removable.is_empty() && kept.is_empty() && temp.is_empty() {
        println!("Nothing to clean up.");
        return Ok(0);
    }

    let verb = if apply { "Removed" } else { "Would remove" };
    if !removable.is_empty() || !temp.is_empty() {
        println!("{verb}:");
        for item in &removable {
            println!("  {} ({})", item.label, what(item));
        }
        if !temp.is_empty() {
            println!(
                "  {} leftover temp index file{}",
                temp.len(),
                if temp.len() == 1 { "" } else { "s" }
            );
        }
    }
    if apply {
        for item in &removable {
            repo.git()
                .run(["update-ref", "-d", &item.reference, &item.oid])?;
        }
        for path in &temp {
            let _ = fs::remove_file(path);
        }
        if !removable.is_empty() {
            println!("\nNo files and no branches were changed. To bring a removed chain back before `git gc` runs:");
            for item in &removable {
                println!("  git update-ref {} {}", item.reference, item.oid);
            }
        }
    }
    if !kept.is_empty() {
        println!("\nKept, because they still hold stones:");
        for item in &kept {
            println!("  {} ({})", item.label, what(item));
            println!(
                "    delete: git stone prune --chain {} --apply",
                item.reference
            );
            if item.reference.starts_with(HEADS) {
                let name = item.reference.trim_start_matches(HEADS);
                println!("    or move them to the branch you are on: git stone adopt {name}");
            }
        }
    }
    if !apply && (!removable.is_empty() || !temp.is_empty()) {
        match chain {
            Some(target) => println!("\nRun `git stone prune --chain {target} --apply` to do it."),
            None => println!("\nRun `git stone prune --apply` to do it."),
        }
    }
    Ok(0)
}

fn what(item: &Leftover) -> String {
    if let Some(why) = &item.unreadable {
        return format!(
            "cannot be read: {}",
            printable(why).lines().next().unwrap_or("")
        );
    }
    match item.stones.as_slice() {
        [] => "no stones".to_string(),
        names => format!(
            "{} stone{}: {}",
            names.len(),
            if names.len() == 1 { "" } else { "s" },
            names
                .iter()
                .map(|n| printable(n))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

/// Every chain whose owner is gone, in ref order. The chain of the current scope is never one.
fn leftovers(repo: &Repo) -> Result<Vec<Leftover>, CmdError> {
    let git = repo.git();
    // Ref names cannot contain spaces or newlines, so `<ref> <oid>` lines are exact.
    let listing = |prefix: &str| -> Result<Vec<(String, String)>, CmdError> {
        let out = git.run(["for-each-ref", "--format=%(refname) %(objectname)", prefix])?;
        Ok(String::from_utf8_lossy(&out)
            .lines()
            .filter_map(|l| l.split_once(' '))
            .map(|(r, o)| (r.to_string(), o.to_string()))
            .collect())
    };
    let branches: HashSet<String> = listing("refs/heads/")?
        .into_iter()
        .map(|(r, _)| r)
        .collect();
    let common = String::from_utf8_lossy(&git.run([
        "rev-parse",
        "--path-format=absolute",
        "--git-common-dir",
    ])?)
    .trim()
    .to_string();
    // Linked worktrees that exist, written the way their ids appear in ref names.
    let live: HashSet<String> = fs::read_dir(std::path::Path::new(&common).join("worktrees"))
        .map(|dir| {
            dir.flatten()
                .map(|e| {
                    format!(
                        "linked/{}",
                        encode_worktree_name(&e.file_name().to_string_lossy())
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    let current = repo.scope.state_ref();

    let mut out = Vec::new();
    for (reference, oid) in listing(HEADS)? {
        let branch = reference.replacen("refs/stones/", "refs/", 1);
        if reference == current || branches.contains(&branch) {
            continue;
        }
        let label = format!("branch `{}`", branch.trim_start_matches("refs/heads/"));
        out.push(describe(repo, reference, label, oid, &branch));
    }
    for (reference, oid) in listing(DETACHED)? {
        let id = reference.trim_start_matches(DETACHED);
        if reference == current || id == "main" || live.contains(id) || !id.starts_with("linked/") {
            continue;
        }
        let label = format!("detached chain of removed worktree `{id}`");
        out.push(describe(repo, reference, label, oid, "detached"));
    }
    Ok(out)
}

fn describe(repo: &Repo, reference: String, label: String, oid: String, branch: &str) -> Leftover {
    let (stones, unreadable) = match State::load_ref(repo, &reference, branch) {
        Ok(Some((_, state))) => (state.stones.into_iter().map(|s| s.name).collect(), None),
        Ok(None) => (Vec::new(), Some("it disappeared".to_string())),
        Err(e) => (Vec::new(), Some(e.to_string())),
    };
    Leftover {
        reference,
        label,
        oid,
        stones,
        unreadable,
    }
}
