use super::{find_stone, load_state, sync_chain, CmdResult};
use crate::failpoint::failpoint;
use crate::repo::Repo;
use crate::snapshot;

/// Remove a boundary. No file changes: the next stone's diff just grows to cover the dropped one (I4).
pub fn run(repo: &Repo, id: u64) -> CmdResult {
    snapshot::clean_stale(repo);
    // A merge or rebase in progress is fine: this only touches the state ref.
    let Some((old, state)) = load_state(repo)? else {
        find_stone(None, id)?;
        unreachable!("find_stone fails without a state");
    };
    // Stones that are already committed on the branch are marked so first.
    let (old, mut state) = sync_chain(repo, old, state)?;
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
