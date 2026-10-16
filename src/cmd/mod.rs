//! One file per command, plus the pieces they share.

pub mod adopt;
pub mod commit;
pub mod create;
pub mod diff;
pub mod drop;
pub mod list;
pub mod prune;
pub mod repair;
pub mod review;
pub mod setup;
pub mod show;
pub mod status;

use crate::chain::{self, Movement, Outcome, SyncError};
use crate::git::GitError;
use crate::repo::{Repo, RepoError, Scope};
use crate::snapshot::SnapshotError;
use crate::state::{State, StateError, Stone};

/// A command that stopped. `code` is the exit code (1 = user can fix it, 2 = internal).
#[derive(Debug)]
pub struct CmdError {
    pub code: i32,
    pub message: String,
}

impl CmdError {
    pub fn user(message: impl Into<String>) -> CmdError {
        CmdError {
            code: 1,
            message: message.into(),
        }
    }
}

impl From<RepoError> for CmdError {
    fn from(e: RepoError) -> CmdError {
        CmdError {
            code: e.exit_code(),
            message: e.to_string(),
        }
    }
}

impl From<StateError> for CmdError {
    fn from(e: StateError) -> CmdError {
        CmdError {
            code: e.exit_code(),
            message: e.to_string(),
        }
    }
}

impl From<SnapshotError> for CmdError {
    fn from(e: SnapshotError) -> CmdError {
        CmdError {
            code: e.exit_code(),
            message: e.to_string(),
        }
    }
}

impl From<GitError> for CmdError {
    fn from(e: GitError) -> CmdError {
        CmdError {
            code: 2,
            message: e.to_string(),
        }
    }
}

/// `Ok(code)` is the exit code of a command that ran (a diff passes git's own code through).
pub type CmdResult = Result<i32, CmdError>;

pub fn load_state(repo: &Repo) -> Result<Option<(String, State)>, CmdError> {
    Ok(State::load(repo)?)
}

/// Find a stone by id. Gives the position in the chain too.
pub fn find_stone(state: Option<&State>, id: u64) -> Result<(usize, &Stone), CmdError> {
    let Some(state) = state.filter(|s| !s.stones.is_empty()) else {
        return Err(CmdError::user(format!(
            "There is no Stone {id}: no stones have been made here yet."
        )));
    };
    match state.stones.iter().position(|s| s.id == id) {
        Some(index) => Ok((index, &state.stones[index])),
        None => Err(CmdError::user(format!(
            "There is no Stone {id}. Run `git stone list` to see the stones."
        ))),
    }
}

/// Text from a stone description is untrusted: drop terminal control codes, keep newlines and tabs.
pub fn printable(text: &str) -> String {
    text.chars()
        .filter(|c| *c == '\n' || *c == '\t' || !c.is_control())
        .collect()
}

/// A description under a heading line, indented.
pub fn indented(text: &str, indent: &str) -> String {
    printable(text)
        .lines()
        .map(|line| {
            if line.is_empty() {
                String::new()
            } else {
                format!("{indent}{line}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn short(oid: &str) -> &str {
    &oid[..oid.len().min(7)]
}

/// `feature/auth` for a branch, `detached HEAD` otherwise.
pub fn scope_label(repo: &Repo) -> String {
    match &repo.scope {
        Scope::Branch(full) => full.strip_prefix("refs/heads/").unwrap_or(full).to_string(),
        Scope::Detached(_) => "detached HEAD".to_string(),
    }
}

/// Where the branch (or detached HEAD) points now, for messages.
fn head_text(repo: &Repo) -> String {
    repo.head
        .as_deref()
        .map_or("(no commits yet)".to_string(), |h| short(h).to_string())
}

fn moved_details(repo: &Repo, state: &State) -> String {
    let base = state
        .anchor
        .as_deref()
        .map_or("(no commits yet)", |a| short(a));
    format!(
        "  Stones are based on commit {base}.\n  The branch is now at   {}.",
        head_text(repo)
    )
}

fn place(repo: &Repo) -> String {
    match &repo.scope {
        Scope::Branch(_) => format!("branch `{}`", scope_label(repo)),
        Scope::Detached(_) => "HEAD".to_string(),
    }
}

/// The message a command that would change things gives when the branch moved (A9).
pub fn moved_refusal(repo: &Repo, state: &State) -> String {
    format!(
        "Git Stone: {} moved since the stones were made.\n{}\nNothing was changed. Run `git stone repair` to see how to fix this.",
        place(repo),
        moved_details(repo, state)
    )
}

/// The first lines of the repair plan: what moved.
pub fn describe_moved(repo: &Repo, state: &State) -> String {
    format!(
        "Git Stone: {} moved since the stones were made.\n{}",
        place(repo),
        moved_details(repo, state)
    )
}

/// The same facts as a warning, for commands that only read.
pub fn moved_warning(repo: &Repo, state: &State) -> String {
    format!(
        "Warning: {} moved since the stones were made.\n{}\n  Run `git stone repair` to see how to fix this.",
        place(repo),
        moved_details(repo, state)
    )
}

/// What a read command says when HEAD is not where the stones were made, if anything.
pub fn movement_warning(repo: &Repo, state: &State) -> Result<Option<String>, CmdError> {
    Ok(match chain::classify(repo, state)? {
        Movement::InSync => None,
        Movement::Moved => Some(moved_warning(repo, state)),
        Movement::Matches(count) => {
            let ids: Vec<u64> = state.stones[..count].iter().map(|s| s.id).collect();
            Some(format!(
                "Warning: {} already on the branch as commits. The next git stone command that changes things marks them as committed.",
                chain::describe_ids(&ids)
            ))
        }
    })
}

/// Read commands never refuse because the branch moved. They say so on stderr and go on.
pub fn warn_if_moved(repo: &Repo, state: Option<&State>) {
    if let Some(state) = state {
        if let Ok(Some(text)) = movement_warning(repo, state) {
            eprintln!("{text}");
        }
    }
}

/// For commands that change things: make sure HEAD is where the stones were made (A9).
/// If the branch only holds commits that equal the first stones, they are marked as committed
/// first. Returns the state commit and state to continue with, or refuses.
pub fn sync_chain(repo: &Repo, oid: String, state: State) -> Result<(String, State), CmdError> {
    match chain::sync(repo, &oid, &state) {
        Ok(Outcome::Unchanged) => Ok((oid, state)),
        Ok(Outcome::Reconciled(done)) => {
            eprintln!("{}", done.note);
            Ok((done.oid, done.state))
        }
        Ok(Outcome::Moved) => Err(CmdError::user(moved_refusal(repo, &state))),
        Err(SyncError::Git(e)) => Err(e.into()),
        Err(SyncError::State(e)) => Err(e.into()),
        Err(SyncError::Index(e)) => Err(CmdError::user(format!(
            "The first stones match commits already on the branch, but your staged changes clash with them, so nothing was changed.\nUnstage the clashing paths (`git restore --staged <path>`) and try again.\n{}",
            e.stderr.trim_end()
        ))),
    }
}

/// A merge, rebase, ... in progress, or conflicted files, make it unsafe to record or commit anything.
pub fn require_clean_start(repo: &Repo, action: &str) -> Result<(), CmdError> {
    if let Some(op) = repo.op_in_progress()? {
        return Err(CmdError::user(format!(
            "A {op} is in progress. Finish or abort it before {action}."
        )));
    }
    if repo.has_unmerged()? {
        return Err(CmdError::user(format!(
            "Some files have merge conflicts. Resolve them before {action}."
        )));
    }
    Ok(())
}

/// Seconds since 1970 as `YYYY-MM-DD HH:MM:SS UTC`.
pub fn utc(seconds: u64) -> String {
    let days = (seconds / 86_400) as i64;
    let rest = seconds % 86_400;
    // Days since 1970-01-01 to a calendar date (Howard Hinnant's civil_from_days).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02} UTC",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utc_dates() {
        assert_eq!(utc(0), "1970-01-01 00:00:00 UTC");
        assert_eq!(utc(951_782_400), "2000-02-29 00:00:00 UTC");
        assert_eq!(utc(1_760_600_000), "2025-10-16 07:33:20 UTC");
        assert_eq!(utc(4_102_444_799), "2099-12-31 23:59:59 UTC");
    }

    #[test]
    fn descriptions_lose_terminal_codes_but_keep_layout() {
        assert_eq!(
            printable("a\u{1b}[31mred\u{7}\tb\nc\u{9b}d"),
            "a[31mred\tb\ncd"
        );
        assert_eq!(indented("one\n\ntwo", "  "), "  one\n\n  two");
    }
}
