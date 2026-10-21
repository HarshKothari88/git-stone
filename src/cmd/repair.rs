use super::{
    describe_moved, load_state, require_clean_start, short, sync_chain, CmdError, CmdResult,
};
use crate::chain::{self, Movement, Replay};
use crate::failpoint::failpoint;
use crate::repo::Repo;

/// Put the stones back on top of the commit the branch is at now (A9). Without `--apply` it only
/// checks the replay and prints what would happen.
pub fn run(repo: &Repo, apply: bool) -> CmdResult {
    chain::require_repair_git().map_err(CmdError::user)?;
    let Some((oid, state)) = load_state(repo)? else {
        println!("There are no stones here, so there is nothing to repair.");
        return Ok(0);
    };
    match chain::classify(repo, &state)? {
        Movement::InSync => {
            println!("Nothing to repair: the branch is where the stones were made.");
            return Ok(0);
        }
        Movement::Matches(count) => {
            let ids: Vec<u64> = state.stones[..count].iter().map(|s| s.id).collect();
            if !apply {
                println!(
                    "Nothing to repair: {} already on the branch as commits.\nThe next git stone command that changes things marks them as committed.",
                    chain::describe_ids(&ids)
                );
                return Ok(0);
            }
            require_clean_start(repo, "repairing")?;
            sync_chain(repo, oid, state)?;
            println!("Nothing else to repair.");
            return Ok(0);
        }
        Movement::Moved => {}
    }

    if apply {
        require_clean_start(repo, "repairing")?;
    }
    let Some(head) = repo.head.as_deref() else {
        return Err(CmdError::user(
            "The branch has no commits now, so there is nothing to put the stones on.\nNothing was changed.",
        ));
    };
    let trees = match chain::replay(repo, &state)? {
        Replay::Clean(trees) => trees,
        Replay::Conflict { index, paths } => {
            let stone = &state.stones[index];
            let mut text = format!(
                "Stone {} ({}) cannot be put on top of the branch's current commit.\nIt conflicts with the branch in:",
                stone.id, stone.name
            );
            for path in &paths {
                text.push_str(&format!("\n  {}", super::printable(path)));
            }
            text.push_str(&format!(
                "\nNothing was changed. Your files are untouched.\nIf the branch already holds this work (for example it was committed with a plain `git commit`), remove the stone: `git stone drop {}`, or `git stone drop all`.\nOtherwise, moving the branch back to commit {} makes the stones usable again.",
                stone.id,
                state.anchor.as_deref().map_or("(no commits yet)", short)
            ));
            return Err(CmdError::user(text));
        }
    };

    // Only stones whose snapshot really differs get a new one.
    let changed: Vec<u64> = state
        .stones
        .iter()
        .zip(&trees)
        .filter(|(stone, tree)| stone.tree != **tree)
        .map(|(stone, _)| stone.id)
        .collect();
    let made = if changed.is_empty() {
        "No snapshot changes: the stones only move onto the new commit.".to_string()
    } else {
        format!(
            "This makes new snapshots for {}. The old ones stay in the reflog of {}.",
            chain::describe_ids(&changed),
            repo.scope.state_ref()
        )
    };

    if !apply {
        println!(
            "{}\n\nRepair puts the stones back on top of the branch's current commit, using Git's own 3-way merge.\nThe check found no conflicts.\n{made}\nNo files are changed.\n\nRun `git stone repair --apply` to do it.",
            describe_moved(repo, &state)
        );
        return Ok(0);
    }

    // Same ids, names, descriptions and times; new trees; the anchor is where the branch is now.
    let mut next = state.clone();
    for (stone, tree) in next.stones.iter_mut().zip(trees) {
        stone.tree = tree;
    }
    next.anchor = Some(head.to_string());
    failpoint("repair:before-cas");
    next.save(repo, Some(&oid), "repair")?;
    println!(
        "Repaired: the stones now sit on commit {}.\n{made}\nNo files were changed.",
        short(head)
    );
    Ok(0)
}
