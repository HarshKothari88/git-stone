use std::io::Write;

use super::{find_stone, indented, load_state, short, utc, warn_if_moved, CmdResult};
use crate::chain;
use crate::repo::Repo;

/// Header, description, then git's own `--stat` and patch for the stone.
pub fn run(repo: &Repo, id: u64) -> CmdResult {
    let loaded = load_state(repo)?;
    let state = loaded.as_ref().map(|(_, s)| s);
    warn_if_moved(repo, state);
    let (index, stone) = find_stone(state, id)?;
    let state = state.expect("stone found");
    let base = chain::base_of(repo, state, index)?;

    println!("Stone {}: {}", stone.id, stone.name);
    println!("Created: {}", utc(stone.created));
    println!("Base: {}", base_label(state, index));
    if let Some(text) = &stone.description {
        println!("\n{}", indented(text, "  "));
    }
    println!();
    // Our text must be out before git (and its pager) starts.
    let _ = std::io::stdout().flush();

    let status = repo
        .git()
        .run_inherit(["diff", "--stat", "--patch", &base, &stone.tree])?;
    Ok(status.code().unwrap_or(2))
}

pub fn base_label(state: &crate::state::State, index: usize) -> String {
    match index.checked_sub(1) {
        Some(prev) => format!("Stone {}", state.stones[prev].id),
        None => match &state.anchor {
            Some(anchor) => format!("branch commit {}", short(anchor)),
            None => "the start (no commits yet)".to_string(),
        },
    }
}
