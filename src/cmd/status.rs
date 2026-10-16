use std::collections::HashSet;
use std::path::Path;

use super::{load_state, moved_warning, movement_warning, scope_label, short, CmdResult};
use crate::chain::{self, Change, Movement};
use crate::repo::{Repo, Scope};
use crate::snapshot;
use crate::state::{self, State, Stone};

const MAX_CHANGES: usize = 50;

/// A warning: a short code for scripts, and the text for people.
struct Warning {
    code: &'static str,
    text: String,
}

fn warning(code: &'static str, text: impl Into<String>) -> Warning {
    Warning {
        code,
        text: text.into(),
    }
}

pub fn run(repo: &Repo, porcelain: bool) -> CmdResult {
    let loaded = load_state(repo)?;
    let state = loaded.as_ref().map(|(_, s)| s);
    let stones: &[Stone] = state.map_or(&[], |s| &s.stones);
    let git = repo.git();

    let current = snapshot::snapshot(repo)?;
    let base = chain::current_base(repo, state)?;
    let changes = chain::name_status(&git, &base, &current)?;
    let warnings = collect_warnings(repo, state, &current)?;

    if porcelain {
        write_porcelain_header("status");
        print!("scope\t{}\0", state::branch_field(&repo.scope));
        print!("head\t{}\0", repo.head.as_deref().unwrap_or("none"));
        print!(
            "anchor\t{}\0",
            state.and_then(|s| s.anchor.as_deref()).unwrap_or("none")
        );
        for stone in stones {
            write_porcelain_stone(stone);
        }
        for change in &changes {
            // A rename is `R\told -> new`; the path field is for display, not for parsing.
            print!("change\t{}\t{}\0", change.code, change.path);
        }
        for w in &warnings {
            print!("warning\t{}\t{}\0", w.code, w.text.replace('\n', " "));
        }
        return Ok(0);
    }

    match &repo.scope {
        Scope::Branch(_) if repo.head.is_none() => {
            println!("Branch: {} (no commits yet)", scope_label(repo))
        }
        Scope::Branch(_) => println!("Branch: {}", scope_label(repo)),
        Scope::Detached(_) => println!(
            "Detached HEAD at {} (stones stay with this worktree)",
            short(repo.head.as_deref().unwrap_or_default())
        ),
    }

    println!("\nStones:");
    if stones.is_empty() {
        println!("  none");
    }
    for stone in stones {
        println!("  {}  {}", stone.id, stone.name);
    }

    let since = match stones.last() {
        Some(last) => format!("Stone {}", last.id),
        None => match &repo.head {
            Some(head) => format!("branch commit {}", short(head)),
            None => "the start (no commits yet)".to_string(),
        },
    };
    println!("\nCurrent changes since {since}:");
    if changes.is_empty() {
        println!("  clean");
    }
    for Change { code, path } in changes.iter().take(MAX_CHANGES) {
        println!("  {code} {path}");
    }
    if changes.len() > MAX_CHANGES {
        println!("  ... and {} more", changes.len() - MAX_CHANGES);
    }
    if !changes.is_empty() {
        let noun = if changes.len() == 1 { "file" } else { "files" };
        println!("\nUnstoned changes: {} {noun}", changes.len());
    }

    if !warnings.is_empty() {
        println!("\nWarnings:");
        for w in &warnings {
            let mut lines = w.text.lines();
            println!("  - {}", lines.next().unwrap_or_default());
            for line in lines {
                println!("    {}", line.trim_start());
            }
        }
    }
    Ok(0)
}

fn collect_warnings(
    repo: &Repo,
    state: Option<&State>,
    current: &str,
) -> Result<Vec<Warning>, super::CmdError> {
    let mut out = Vec::new();
    if let Some(state) = state {
        match chain::classify(repo, state)? {
            Movement::InSync => {}
            Movement::Moved => out.push(warning(
                "branch-moved",
                moved_warning(repo, state).replacen("Warning: ", "", 1),
            )),
            Movement::Matches(_) => out.push(warning(
                "stones-already-committed",
                movement_warning(repo, state)?
                    .unwrap_or_default()
                    .replacen("Warning: ", "", 1),
            )),
        }
    }
    if let Some(op) = repo.op_in_progress()? {
        out.push(warning(
            "operation-in-progress",
            format!("A {op} is in progress."),
        ));
    }
    if repo.has_unmerged()? {
        out.push(warning(
            "unmerged-paths",
            "Some files have merge conflicts.",
        ));
    }
    if let Some(state) = state {
        if let Some(last) = state.stones.last() {
            if current == chain::anchor_tree(repo, state)? {
                out.push(warning(
                    "matches-branch-commit",
                    format!(
                        "Your working tree matches the branch commit, not Stone {}. Did you stash?",
                        last.id
                    ),
                ));
            }
        }
    }
    for path in dirty_submodules(repo)? {
        out.push(warning(
            "dirty-submodule",
            format!("Submodule `{path}` has uncommitted changes; stones do not capture them."),
        ));
    }
    for name in orphaned_chains(repo)? {
        out.push(warning(
            "orphaned-chain",
            format!("Stones exist for branch `{name}`, which is gone. If it was renamed, run `git stone adopt {name}`."),
        ));
    }
    if detached_chain_fits_here(repo, state) {
        out.push(warning(
            "detached-chain",
            "This worktree has stones made on a detached HEAD at this commit. To move them here, run `git stone adopt detached`.",
        ));
    }
    if let Scope::Branch(full) = &repo.scope {
        if checked_out_count(repo, full)? > 1 {
            out.push(warning(
                "branch-in-two-worktrees",
                format!(
                    "Branch `{}` is checked out in more than one worktree.",
                    scope_label(repo)
                ),
            ));
        }
    }
    Ok(out)
}

/// Paths of submodules whose own working tree has changes. Only looked at when `.gitmodules` exists.
fn dirty_submodules(repo: &Repo) -> Result<Vec<String>, super::CmdError> {
    if !repo.top.join(".gitmodules").exists() {
        return Ok(Vec::new());
    }
    let listing = repo.git().run(["ls-files", "--stage", "-z"])?;
    let mut dirty = Vec::new();
    for record in listing.split(|b| *b == 0).filter(|r| !r.is_empty()) {
        let text = String::from_utf8_lossy(record);
        let Some((info, path)) = text.split_once('\t') else {
            continue;
        };
        if !info.starts_with("160000 ") || !Path::new(&repo.top).join(path).join(".git").exists() {
            continue;
        }
        let inside = crate::git::Git::new().in_dir(repo.top.join(path));
        if !inside.run(["status", "--porcelain", "-z"])?.is_empty() {
            dirty.push(path.to_string());
        }
    }
    Ok(dirty)
}

/// Branch names that have a chain with stones but no branch any more.
fn orphaned_chains(repo: &Repo) -> Result<Vec<String>, super::CmdError> {
    let git = repo.git();
    let list = |prefix: &str| -> Result<Vec<String>, super::CmdError> {
        let out = git.run(["for-each-ref", "--format=%(refname)%00", prefix])?;
        Ok(out
            .split(|b| *b == 0)
            .filter(|r| !r.is_empty())
            .map(|r| {
                String::from_utf8_lossy(r)
                    .trim_start_matches('\n')
                    .to_string()
            })
            .collect())
    };
    let chains = list("refs/stones/heads/")?;
    if chains.is_empty() {
        return Ok(Vec::new());
    }
    let branches: HashSet<String> = list("refs/heads/")?.into_iter().collect();
    let mut orphans = Vec::new();
    for chain_ref in chains {
        let branch = chain_ref.replacen("refs/stones/", "refs/", 1);
        if branches.contains(&branch) {
            continue;
        }
        // A chain we cannot read is not worth failing status for.
        if let Ok(Some((_, state))) = State::load_ref(repo, &chain_ref, &branch) {
            if !state.stones.is_empty() {
                orphans.push(branch.trim_start_matches("refs/heads/").to_string());
            }
        }
    }
    Ok(orphans)
}

/// On a branch without stones: does this worktree's detached chain start at the commit we are on?
fn detached_chain_fits_here(repo: &Repo, state: Option<&State>) -> bool {
    if !matches!(repo.scope, Scope::Branch(_)) || state.is_some_and(|s| !s.stones.is_empty()) {
        return false;
    }
    let Ok(Some((_, detached))) = State::load_ref(
        repo,
        &Scope::Detached(repo.worktree_id.clone()).state_ref(),
        "detached",
    ) else {
        return false;
    };
    !detached.stones.is_empty() && detached.anchor == repo.head
}

/// How many worktrees have this branch checked out.
fn checked_out_count(repo: &Repo, branch: &str) -> Result<usize, super::CmdError> {
    let out = repo.git().run(["worktree", "list", "--porcelain", "-z"])?;
    let wanted = format!("branch {branch}");
    Ok(out
        .split(|b| *b == 0)
        .filter(|field| *field == wanted.as_bytes())
        .count())
}

/// Machine output starts with the format name and version, so a script can refuse a format it does not know.
pub fn write_porcelain_header(command: &str) {
    print!("git-stone-porcelain\t1\t{command}\0");
}

/// `stone<TAB>id<TAB>created<TAB>tree<TAB>name<TAB>description`. The description is last and may hold tabs and newlines, never NUL.
pub fn write_porcelain_stone(stone: &Stone) {
    print!(
        "stone\t{}\t{}\t{}\t{}\t{}\0",
        stone.id,
        stone.created,
        stone.tree,
        stone.name,
        stone.description.as_deref().unwrap_or("")
    );
}
