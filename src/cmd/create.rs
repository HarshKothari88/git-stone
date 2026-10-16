use std::time::{SystemTime, UNIX_EPOCH};

use super::{indented, load_state, moved_refusal, CmdError, CmdResult};
use crate::chain;
use crate::failpoint::failpoint;
use crate::repo::Repo;
use crate::snapshot;
use crate::state::{self, State, Stone};

/// More new paths than this in one stone is probably a missing `.gitignore` entry.
const MANY_NEW_PATHS: usize = 1000;

pub fn run(
    repo: &Repo,
    name: &str,
    description: Option<&str>,
    allow_empty: bool,
    force: bool,
) -> CmdResult {
    let name = state::validate_name(name).map_err(CmdError::user)?;
    let description = match description {
        Some(text) => state::validate_description(text).map_err(CmdError::user)?,
        None => None,
    };

    snapshot::clean_stale(repo);
    if let Some(op) = repo.op_in_progress()? {
        return Err(CmdError::user(format!(
            "A {op} is in progress. Finish or abort it before making a stone."
        )));
    }
    if repo.has_unmerged()? {
        return Err(CmdError::user(
            "Some files have merge conflicts. Resolve them before making a stone, or the conflict markers would be saved.",
        ));
    }

    let (old, mut state) = match load_state(repo)? {
        Some((oid, state)) => (Some(oid), state),
        None => (
            None,
            State::new(
                state::branch_field(&repo.scope).to_string(),
                repo.head.clone(),
            ),
        ),
    };
    if state.stones.is_empty() {
        // Nothing can be misattributed yet, so the chain starts wherever HEAD is now.
        state.anchor = repo.head.clone();
    } else if repo.head != state.anchor && !chain::try_reconcile(repo, &state)? {
        return Err(CmdError::user(moved_refusal(repo, &state)));
    }

    let tree = snapshot::snapshot(repo)?;
    let base = chain::latest_tree(repo, &state)?;
    let git = repo.git();

    if tree == base && !allow_empty {
        return Err(CmdError::user(match state.stones.last() {
            Some(last) => format!("Nothing changed since Stone {}.", last.id),
            None if state.anchor.is_some() => {
                "Nothing changed since the branch commit.".to_string()
            }
            None => "Nothing to stone yet: the working tree is empty.".to_string(),
        }));
    }
    if let Some(last) = state.stones.last() {
        if !force && tree == chain::anchor_tree(repo, &state)? {
            return Err(CmdError::user(format!(
                "Your working tree matches the branch commit, not Stone {}. Did you stash? Use --force to stone it anyway.",
                last.id
            )));
        }
    }
    let added = chain::count_added(&git, &base, &tree)?;
    if added > MANY_NEW_PATHS {
        eprintln!(
            "Warning: this stone adds {added} new paths. Is something missing from .gitignore (node_modules, build output)?"
        );
    }

    let id = state.next_id;
    let created = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    state.stones.push(Stone {
        id,
        created,
        name: name.clone(),
        description: description.clone(),
        tree: tree.clone(),
    });
    state.next_id += 1;

    failpoint("create:before-cas");
    state.save(repo, old.as_deref(), &format!("create {id}"))?;

    println!("Stone {id} created: {name}");
    println!("  {}", chain::summary(&chain::numstat(&git, &base, &tree)?));
    if let Some(text) = &description {
        println!("{}", indented(text, "  "));
    }
    Ok(0)
}
