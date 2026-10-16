use std::fs;
use std::path::{Path, PathBuf};

use super::{CmdError, CmdResult};
use crate::repo::Repo;
use crate::AGENT_MEMORY;

/// The lines that mark the block we own. Everything outside them is the user's and is never touched.
const BEGIN: &str =
    "<!-- git-stone:begin (managed by `git stone setup`; edit outside these lines) -->";
const END: &str = "<!-- git-stone:end -->";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Agent {
    Claude,
    Codex,
    Both,
}

/// Put the Git Stone block in the memory file of an AI agent: `CLAUDE.md` or `AGENTS.md` of this
/// project, or with `global` the one in the user's own agent folder.
pub fn run(agent: Agent, global: bool) -> CmdResult {
    let targets: Vec<(&str, PathBuf)> = match agent {
        Agent::Claude => vec![("Claude Code", file(true, global)?)],
        Agent::Codex => vec![("Codex", file(false, global)?)],
        Agent::Both => vec![
            ("Claude Code", file(true, global)?),
            ("Codex", file(false, global)?),
        ],
    };
    // Work out everything first, so a refusal for one file changes none of them.
    let mut plans = Vec::new();
    for (name, path) in &targets {
        let existing = match fs::read(path) {
            Ok(bytes) => Some(String::from_utf8(bytes).map_err(|_| {
                CmdError::user(format!(
                    "{} is not valid UTF-8, so nothing was changed.",
                    path.display()
                ))
            })?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => {
                return Err(CmdError::user(format!(
                    "Cannot read {}: {e}. Nothing was changed.",
                    path.display()
                )))
            }
        };
        let merged = merge(existing.as_deref().unwrap_or(""), AGENT_MEMORY).map_err(|why| {
            CmdError::user(format!("{}: {why} Nothing was changed.", path.display()))
        })?;
        plans.push((*name, path, existing.is_some(), merged));
    }
    for (name, path, existed, merged) in plans {
        match merged {
            None => println!("{name}: already up to date ({})", path.display()),
            Some(text) => {
                if let Some(dir) = path.parent() {
                    fs::create_dir_all(dir).map_err(|e| write_error(path, e))?;
                }
                // Written in place, so a symlinked CLAUDE.md or AGENTS.md keeps pointing where it did.
                fs::write(path, text).map_err(|e| write_error(path, e))?;
                let what = if existed { "updated" } else { "created" };
                println!("{name}: {what} {}", path.display());
            }
        }
    }
    println!("Your other instructions were not changed. Agents read this file when they start a new session.");
    Ok(0)
}

fn write_error(path: &Path, e: std::io::Error) -> CmdError {
    CmdError {
        code: 2,
        message: format!("cannot write {}: {e}", path.display()),
    }
}

/// `CLAUDE.md` (claude) or `AGENTS.md`, in the project or in the agent's own folder.
fn file(claude: bool, global: bool) -> Result<PathBuf, CmdError> {
    let name = if claude { "CLAUDE.md" } else { "AGENTS.md" };
    if !global {
        let repo = Repo::discover().map_err(|e| {
            CmdError::user(format!(
                "{e}\nRun this inside the project, or add --global to set it up for every project."
            ))
        })?;
        return Ok(repo.top.join(name));
    }
    let (variable, folder) = if claude {
        ("CLAUDE_CONFIG_DIR", ".claude")
    } else {
        ("CODEX_HOME", ".codex")
    };
    if let Some(dir) = std::env::var_os(variable).filter(|d| !d.is_empty()) {
        return Ok(PathBuf::from(dir).join(name));
    }
    match std::env::var_os("HOME").filter(|h| !h.is_empty()) {
        Some(home) => Ok(PathBuf::from(home).join(folder).join(name)),
        None => Err(CmdError::user(
            "Cannot find your home folder (HOME is not set), so --global cannot say where to write.",
        )),
    }
}

/// The text of the file after adding (or refreshing) our block, or `None` when it is already right.
/// An existing block is replaced in place; otherwise it is added at the end after a blank line.
fn merge(existing: &str, block: &str) -> Result<Option<String>, String> {
    let ours = format!("{BEGIN}\n{}\n{END}\n", block.trim_end());
    let begins = existing.matches("<!-- git-stone:begin").count();
    let ends = existing.matches(END).count();
    if begins == 0 && ends == 0 {
        let mut text = existing.to_string();
        if !text.is_empty() {
            if !text.ends_with('\n') {
                text.push('\n');
            }
            text.push('\n');
        }
        text.push_str(&ours);
        return Ok(Some(text));
    }
    if begins != 1 || ends != 1 {
        return Err("it has a damaged Git Stone block (a begin or end line is missing or repeated); fix or remove it by hand.".into());
    }
    let start = existing.find("<!-- git-stone:begin").unwrap_or(0);
    let stop = existing.find(END).unwrap_or(0);
    if stop < start {
        return Err("its Git Stone block ends before it begins; fix or remove it by hand.".into());
    }
    let mut after = stop + END.len();
    if existing[after..].starts_with('\n') {
        after += 1;
    }
    let text = format!("{}{ours}{}", &existing[..start], &existing[after..]);
    Ok((text != existing).then_some(text))
}

#[cfg(test)]
mod tests {
    use super::*;

    const BLOCK: &str = "## Git Stone\nUse stones.\n";

    #[test]
    fn a_new_file_gets_just_the_block() {
        let text = merge("", BLOCK).unwrap().unwrap();
        assert_eq!(text, format!("{BEGIN}\n## Git Stone\nUse stones.\n{END}\n"));
    }

    #[test]
    fn existing_text_is_kept_byte_for_byte_and_the_block_goes_last() {
        for existing in [
            "# Mine\nrules\n",
            "# Mine\nno newline at the end",
            "\n\n  odd\n\n",
        ] {
            let text = merge(existing, BLOCK).unwrap().unwrap();
            assert!(text.starts_with(existing), "{text:?}");
            assert!(text.ends_with(&format!("{END}\n")));
            assert_eq!(text.matches("git-stone:begin").count(), 1);
        }
    }

    #[test]
    fn running_twice_changes_nothing() {
        let once = merge("# Mine\n", BLOCK).unwrap().unwrap();
        assert_eq!(merge(&once, BLOCK).unwrap(), None);
    }

    #[test]
    fn an_old_block_is_replaced_in_place_with_the_text_around_it_kept() {
        let old = format!("before\n\n{BEGIN}\nOLD TEXT\n{END}\n\nafter\n");
        let text = merge(&old, BLOCK).unwrap().unwrap();
        assert_eq!(
            text,
            format!("before\n\n{BEGIN}\n## Git Stone\nUse stones.\n{END}\n\nafter\n")
        );
        assert!(!text.contains("OLD TEXT"));
    }

    #[test]
    fn a_damaged_block_is_refused() {
        for bad in [
            format!("{BEGIN}\nno end\n"),
            format!("no begin\n{END}\n"),
            format!("{END}\n{BEGIN}\n"),
            format!("{BEGIN}\n{END}\n{BEGIN}\n{END}\n"),
        ] {
            assert!(merge(&bad, BLOCK).is_err(), "{bad:?}");
        }
    }
}
