use super::{find_stone, load_state, sync_chain, CmdError, CmdResult};
use crate::chain::{self, Movement};
use crate::failpoint::failpoint;
use crate::repo::Repo;
use crate::snapshot;
use crate::state::State;

/// Remove a boundary. No file changes: the next stone's diff just grows to cover the dropped one (I4).
pub fn run(repo: &Repo, id: u64) -> CmdResult {
    snapshot::clean_stale(repo);
    // A merge or rebase in progress is fine: this only touches the state ref.
    let Some((old, state)) = load_state(repo)? else {
        find_stone(None, id)?;
        unreachable!("find_stone fails without a state");
    };
    let (old, mut state) = sync_for_drop(repo, old, state)?;
    let (index, dropped) = find_stone(Some(&state), id)?;

    let (dropped_id, dropped_name) = (dropped.id, dropped.name.clone());
    // Everything else stays as it is, `next_id` included, so ids are never reused (I7).
    state.stones.remove(index);
    failpoint("drop:before-cas");
    // The chain stays even when it is now empty: the ref also remembers `next_id`.
    state.save(repo, Some(&old), &format!("drop {dropped_id}"))?;

    println!("Dropped Stone {dropped_id}: {dropped_name}\n\nNo files were changed.");
    match state.stones.get(index) {
        Some(next) => println!(
            "Stone {} now includes all changes from the old Stone {dropped_id} boundary.",
            next.id
        ),
        None => println!(
            "Your unstoned changes now include all changes from the old Stone {dropped_id} boundary."
        ),
    }
    Ok(0)
}

/// Stones that are already committed on the branch are marked so first. When the branch moved for any
/// other reason the chain is left as it is: dropping only deletes a record and touches no file and no
/// commit, so it must stay possible. It is the way out when `repair` cannot replay the stones.
fn sync_for_drop(repo: &Repo, old: String, state: State) -> Result<(String, State), CmdError> {
    match chain::classify(repo, &state)? {
        Movement::Moved => {
            eprintln!("Note: the branch moved since the stones were made. Dropping only removes the record.");
            Ok((old, state))
        }
        _ => sync_chain(repo, old, state),
    }
}

/// `git stone drop all`: remove every boundary of this branch. No file changes.
pub fn run_all(repo: &Repo) -> CmdResult {
    snapshot::clean_stale(repo);
    let state = load_state(repo)?.filter(|(_, s)| !s.stones.is_empty());
    let Some((old, state)) = state else {
        return Err(CmdError::user(
            "There are no stones here, so there is nothing to drop.",
        ));
    };
    let (old, mut state) = sync_for_drop(repo, old, state)?;
    let count = state.stones.len();
    if count == 0 {
        println!(
            "Every stone was already committed on the branch, so there is nothing left to drop."
        );
        return Ok(0);
    }
    state.stones.clear();
    failpoint("drop:before-cas");
    // The chain stays: the ref also remembers `next_id`, so ids are never reused (I7).
    state.save(repo, Some(&old), "drop all")?;
    let noun = if count == 1 { "stone" } else { "stones" };
    println!("Dropped {count} {noun}.\n\nNo files were changed. Your unstoned changes now include everything.");
    Ok(0)
}
