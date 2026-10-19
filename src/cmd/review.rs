use super::{exclude, indented, load_state, warn_if_moved, CmdResult};
use crate::chain::{self, FileStat};
use crate::repo::Repo;
use crate::snapshot;

const MAX_FILES: usize = 50;

/// Every stone with its description and size, then what is still unstoned.
pub fn run(repo: &Repo) -> CmdResult {
    let loaded = load_state(repo)?;
    let state = loaded.as_ref().map(|(_, s)| s);
    warn_if_moved(repo, state);
    let git = repo.git();

    if let Some(state) = state {
        for (index, stone) in state.stones.iter().enumerate() {
            let base = chain::base_of(repo, state, index)?;
            println!("Stone {}  {}", stone.id, stone.name);
            if let Some(text) = &stone.description {
                println!("{}", indented(text, "    "));
            }
            print_stats(&chain::numstat(&git, &base, &stone.tree)?);
            println!();
        }
    }

    let base = chain::current_base(repo, state)?;
    let current = snapshot::snapshot(repo, &base, &exclude::read(repo)?)?;
    println!("CURRENT");
    print_stats(&chain::numstat(&git, &base, &current)?);
    Ok(0)
}

fn print_stats(stats: &[FileStat]) {
    println!("    {}", chain::summary(stats));
    for stat in stats.iter().take(MAX_FILES) {
        match (stat.added, stat.deleted) {
            (Some(a), Some(d)) => println!("    +{a} -{d}  {}", stat.path),
            _ => println!("    binary  {}", stat.path),
        }
    }
    if stats.len() > MAX_FILES {
        println!("    ... and {} more", stats.len() - MAX_FILES);
    }
}
