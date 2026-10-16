use super::{load_state, warn_if_moved, CmdResult};
use crate::repo::Repo;

pub fn run(repo: &Repo, porcelain: bool) -> CmdResult {
    let loaded = load_state(repo)?;
    let state = loaded.as_ref().map(|(_, s)| s);
    warn_if_moved(repo, state);
    let stones = state.map_or(&[][..], |s| &s.stones[..]);
    if porcelain {
        super::status::write_porcelain_header("list");
        for stone in stones {
            super::status::write_porcelain_stone(stone);
        }
    } else if stones.is_empty() {
        println!("No stones yet.");
    } else {
        for stone in stones {
            println!("  {}  {}", stone.id, stone.name);
        }
    }
    Ok(0)
}
