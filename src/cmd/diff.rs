use super::{exclude, find_stone, load_state, warn_if_moved, CmdResult};
use crate::chain;
use crate::repo::Repo;
use crate::snapshot;
use crate::Target;

/// `git diff <base> <tree> <your args>`, on your terminal, so pager and colours work.
pub fn run(repo: &Repo, target: &Target, args: &[String]) -> CmdResult {
    let loaded = load_state(repo)?;
    let state = loaded.as_ref().map(|(_, s)| s);
    warn_if_moved(repo, state);
    let (base, tree) = match target {
        Target::Id(id) => {
            let (index, stone) = find_stone(state, *id)?;
            (
                chain::base_of(repo, state.expect("stone found"), index)?,
                stone.tree.clone(),
            )
        }
        Target::Current => {
            let base = chain::current_base(repo, state)?;
            let tree = snapshot::snapshot(repo, &base, &exclude::read(repo)?)?;
            (base, tree)
        }
    };
    let mut git_args = vec!["diff".to_string(), base, tree];
    git_args.extend(args.iter().cloned());
    let status = repo.git().run_inherit(git_args)?;
    Ok(status.code().unwrap_or(2))
}
