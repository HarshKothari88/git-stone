use super::{find_stone, load_state, moved_refusal, CmdError, CmdResult};
use crate::chain;
use crate::failpoint::failpoint;
use crate::repo::Repo;
use crate::snapshot;

/// Remove a boundary. No file changes: the next stone's diff just grows to cover the dropped one (I4).
pub fn run(repo: &Repo, id: u64) -> CmdResult {
    snapshot::clean_stale(repo);
    // A merge or rebase in progress is fine: this only touches the state ref.
    let loaded = load_state(repo)?;
    let (index, dropped) = find_stone(loaded.as_ref().map(|(_, s)| s), id)?;
    let (old, mut state) = loaded.clone().expect("stone found");

    if chain::moved(repo, &state) && !chain::try_reconcile(repo, &state)? {
        return Err(CmdError::user(moved_refusal(repo, &state)));
    }

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
