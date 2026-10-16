use std::time::{SystemTime, UNIX_EPOCH};

use super::{indented, load_state, require_clean_start, sync_chain, CmdError, CmdResult};
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
    require_clean_start(repo, "making a stone")?;

    let (old, mut state) = match load_state(repo)? {
        // Stones whose commits are already on the branch are marked committed first.
        Some((oid, state)) if !state.stones.is_empty() => {
            let (oid, state) = sync_chain(repo, oid, state)?;
            (Some(oid), state)
        }
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
