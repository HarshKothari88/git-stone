use super::{load_state, CmdError, CmdResult};
use crate::repo::{Repo, Scope};
use crate::state::{branch_field, State};

/// Move a chain from `source` (a branch name or `detached`) to the current scope (A10b).
pub fn run(repo: &Repo, source: &str) -> CmdResult {
    let from = source_scope(repo, source)?;
    if from == repo.scope {
        return Err(CmdError::user(format!(
            "The stones of {} are already here.",
            label(&from)
        )));
    }
    let source_ref = from.state_ref();
    let Some((source_old, source_state)) = State::load_ref(repo, &source_ref, branch_field(&from))?
        .filter(|(_, state)| !state.stones.is_empty())
    else {
        return Err(CmdError::user(format!(
            "There are no stones to move from {}.",
            label(&from)
        )));
    };

    let target = load_state(repo)?;
    if target.as_ref().is_some_and(|(_, s)| !s.stones.is_empty()) {
        return Err(CmdError::user(format!(
            "There are already stones on {}, so nothing was moved.\nDrop them first, or work on another branch.",
            label(&repo.scope)
        )));
    }
    let (target_old, target_next_id) = match target {
        Some((oid, state)) => (Some(oid), state.next_id),
        None => (None, 1),
    };

    // Keep the ids when they cannot collide with ids this scope used before (I7); otherwise count on from its next id.
    let keep = source_state.stones.iter().all(|s| s.id >= target_next_id);
    let mut moved = source_state.clone();
    let mut renumbered = Vec::new();
    if !keep {
        let mut next = target_next_id;
        for stone in &mut moved.stones {
            renumbered.push((stone.id, next));
            stone.id = next;
            next += 1;
        }
        moved.next_id = next;
    }
    moved.branch = branch_field(&repo.scope).to_string();
    let count = moved.stones.len();

    moved.save_moved(
        repo,
        target_old.as_deref(),
        &source_ref,
        &source_old,
        &format!("adopt {count} from {}", branch_field(&from)),
    )?;

    println!(
        "Moved {count} {} from {} to {}.",
        if count == 1 { "stone" } else { "stones" },
        label(&from),
        label(&repo.scope)
    );
    for (old, new) in renumbered {
        println!("Stone {old} is now Stone {new}.");
    }
    println!("No files were changed.");
    Ok(0)
}

/// `detached`, a branch name, or a full `refs/heads/...` name (for a branch that is itself called `detached`).
fn source_scope(repo: &Repo, source: &str) -> Result<Scope, CmdError> {
    if source == "detached" {
        return Ok(Scope::Detached);
    }
    let name = source.strip_prefix("refs/heads/").unwrap_or(source);
    let full = format!("refs/heads/{name}");
    if repo.git().run(["check-ref-format", &full]).is_err() {
        return Err(CmdError::user(format!(
            "`{source}` is not a valid branch name."
        )));
    }
    Ok(Scope::Branch(full))
}

fn label(scope: &Scope) -> String {
    match scope {
        Scope::Branch(full) => format!(
            "branch `{}`",
            full.strip_prefix("refs/heads/").unwrap_or(full)
        ),
        Scope::Detached => "detached HEAD".to_string(),
    }
}
