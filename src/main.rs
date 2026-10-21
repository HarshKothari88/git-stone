// Phase 0: commands are stubs, so most of git.rs, repo.rs and failpoint.rs is not called yet.
// Remove this line once the commands land.

mod chain;
mod cmd;
mod failpoint;
mod git;
mod repo;
mod snapshot;
mod state;

use std::process::exit;

const HELP: &str = "\
git stone - save task boundaries as you work, commit them later

  git stone <name> [-d <text>]      create a stone; -d says in detail what it is for
  git stone <name> -x <pattern> [--remember]   leave matching paths (globs work) out of this stone;
                                    --remember keeps the rule for this repository, every branch
  git stone exclude [--remove <pattern> | --clear]   show or change the remembered rules
  git stone -- <name>               create, when the name looks like a command, a number, or starts with -
  git stone                         same as status
  git stone status [--porcelain]
  git stone list [--porcelain]
  git stone review
  git stone diff <id|current> [git diff args]     also: git stone <id> diff
  git stone show <id>                             also: git stone <id> show
  git stone <id> commit [-m msg | -F file | -e | --no-verify | -S | --author | --date | -s]
                                                  also: git stone commit <id> ...
  git stone <id> drop                             also: git stone drop <id>
  git stone drop all                              remove every boundary of this branch
  git stone commit-all [--edit]
  git stone repair [--apply]
  git stone adopt <branch | detached>
  git stone prune [--apply] [--chain <ref>]       list (or remove) chains of deleted branches and worktrees
  git stone agents                                print a block to paste into an AI agent's memory
  git stone guard [block | warn | off]            while a branch has stones, refuse plain git commands that move it
  git stone setup --agent <claude|codex|both> [--global]   add that block to CLAUDE.md / AGENTS.md for you
  git stone setup --update                        refresh the block in files that already have it
  git stone help | version

create flags: --allow-empty, --force
";

/// The block for an AI agent's memory; the same text is shown in docs/ai-agents.md.
const AGENT_MEMORY: &str = include_str!("../docs/agent-memory.md");

/// Words that are commands, so they cannot start a stone name without `--`.
const VERBS: &[&str] = &[
    "status",
    "list",
    "review",
    "diff",
    "show",
    "commit",
    "drop",
    "commit-all",
    "repair",
    "adopt",
    "prune",
    "exclude",
    "guard",
    "guard-check",
    "agents",
    "setup",
    "help",
    "version",
];

#[derive(Debug, PartialEq, Eq)]
enum Target {
    Id(u64),
    Current,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ExcludeAction {
    List,
    Remove(String),
    Clear,
}

#[derive(Debug, PartialEq, Eq)]
pub enum GuardAction {
    Show,
    Block,
    Warn,
    Off,
}

#[derive(Debug, PartialEq, Eq)]
enum Command {
    Create {
        name: String,
        /// Checked and trimmed by the create command, not here.
        description: Option<String>,
        allow_empty: bool,
        force: bool,
        /// Git pathspecs for this stone, as typed. The create command checks them.
        exclude: Vec<String>,
        /// Save `exclude` for later stones in this repository.
        remember: bool,
    },
    Exclude(ExcludeAction),
    Guard(GuardAction),
    /// Called by the guard hook, not by people. `phase` is the hook's first argument.
    GuardCheck {
        phase: String,
    },
    Status {
        porcelain: bool,
    },
    List {
        porcelain: bool,
    },
    Review,
    /// `args` go to `git diff` untouched.
    Diff {
        target: Target,
        args: Vec<String>,
    },
    Show {
        id: u64,
    },
    /// `args` are checked against the allowed list by the commit command itself.
    Commit {
        id: u64,
        args: Vec<String>,
    },
    Drop {
        id: u64,
    },
    DropAll,
    CommitAll {
        edit: bool,
    },
    Repair {
        apply: bool,
    },
    /// A branch name, or the word `detached`.
    Adopt {
        source: String,
    },
    /// `chain` is a full state ref, named explicitly because it may still hold stones.
    Prune {
        apply: bool,
        chain: Option<String>,
    },
    Agents,
    /// Adds the memory block to the agent's own file.
    Setup {
        agent: cmd::setup::Agent,
        global: bool,
    },
    /// Refreshes the block in the memory files that already have it.
    SetupUpdate,
    Help,
    Version,
}

/// Turn the arguments after `git stone` into a command. The error is the message for the user.
fn parse(args: &[String]) -> Result<Command, String> {
    let Some((first, rest)) = args.split_first() else {
        return Ok(Command::Status { porcelain: false });
    };
    match first.as_str() {
        "status" => Ok(Command::Status {
            porcelain: flag("status", rest, "--porcelain")?,
        }),
        "list" => Ok(Command::List {
            porcelain: flag("list", rest, "--porcelain")?,
        }),
        "review" => no_args("review", rest).map(|()| Command::Review),
        "diff" => {
            let (target, passthrough) = rest
                .split_first()
                .ok_or("diff needs a stone number or `current`: git stone diff <id|current>")?;
            let target = if target == "current" {
                Target::Current
            } else {
                Target::Id(parse_id(target)?)
            };
            Ok(Command::Diff {
                target,
                args: passthrough.to_vec(),
            })
        }
        "show" => Ok(Command::Show {
            id: one_id("show", rest)?,
        }),
        "commit" => {
            let (id, passthrough) = rest
                .split_first()
                .ok_or("commit needs a stone number: git stone commit <id>")?;
            Ok(Command::Commit {
                id: parse_id(id)?,
                args: passthrough.to_vec(),
            })
        }
        "drop" => match rest {
            [all] if all == "all" => Ok(Command::DropAll),
            _ => Ok(Command::Drop {
                id: one_id("drop", rest)?,
            }),
        },
        "commit-all" => Ok(Command::CommitAll {
            edit: flag("commit-all", rest, "--edit")?,
        }),
        "repair" => Ok(Command::Repair {
            apply: flag("repair", rest, "--apply")?,
        }),
        "adopt" => match rest {
            [source] => Ok(Command::Adopt {
                source: source.clone(),
            }),
            _ => Err(
                "adopt needs one branch name or `detached`: git stone adopt <branch|detached>"
                    .into(),
            ),
        },
        "prune" => parse_prune(rest),
        "exclude" => parse_exclude(rest),
        "guard" => match rest {
            [] => Ok(Command::Guard(GuardAction::Show)),
            [mode] if mode == "block" => Ok(Command::Guard(GuardAction::Block)),
            [mode] if mode == "warn" => Ok(Command::Guard(GuardAction::Warn)),
            [mode] if mode == "off" => Ok(Command::Guard(GuardAction::Off)),
            _ => Err("guard takes nothing (show), `block`, `warn` or `off`.".into()),
        },
        "guard-check" => match rest {
            [phase] => Ok(Command::GuardCheck {
                phase: phase.clone(),
            }),
            _ => Err(
                "guard-check is called by the guard hook, with the phase as its argument.".into(),
            ),
        },
        "agents" => no_args("agents", rest).map(|()| Command::Agents),
        "setup" => parse_setup(rest),
        "help" => no_args("help", rest).map(|()| Command::Help),
        "version" => no_args("version", rest).map(|()| Command::Version),
        number if is_number(number) => parse_id_first(number, rest),
        _ => parse_create(args),
    }
}

fn parse_setup(rest: &[String]) -> Result<Command, String> {
    if rest.iter().any(|a| a == "--update") {
        return match rest {
            [_only] => Ok(Command::SetupUpdate),
            _ => Err("`setup --update` takes no other option; it refreshes the files that already have the block.".into()),
        };
    }
    let (mut agent, mut global) = (None, false);
    let mut args = rest.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--global" => global = true,
            "--agent" => {
                agent = Some(match args.next().map(String::as_str) {
                    Some("claude") => cmd::setup::Agent::Claude,
                    Some("codex") => cmd::setup::Agent::Codex,
                    Some("both") => cmd::setup::Agent::Both,
                    _ => return Err("--agent needs `claude`, `codex` or `both`.".into()),
                })
            }
            other => {
                return Err(format!(
                    "`setup` does not take `{other}`. See `git stone help`."
                ))
            }
        }
    }
    let agent = agent.ok_or(
        "setup needs --agent: git stone setup --agent <claude|codex|both> [--global]\nTo refresh a block that is already there: git stone setup --update",
    )?;
    Ok(Command::Setup { agent, global })
}

fn parse_exclude(rest: &[String]) -> Result<Command, String> {
    match rest {
        [] => Ok(Command::Exclude(ExcludeAction::List)),
        [flag] if flag == "--clear" => Ok(Command::Exclude(ExcludeAction::Clear)),
        [flag, pattern] if flag == "--remove" => {
            Ok(Command::Exclude(ExcludeAction::Remove(pattern.clone())))
        }
        _ => {
            Err("exclude takes nothing (show the rules), `--remove <pattern>` or `--clear`.".into())
        }
    }
}

fn parse_prune(rest: &[String]) -> Result<Command, String> {
    let (mut apply, mut chain) = (false, None);
    let mut args = rest.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--apply" => apply = true,
            "--chain" => {
                match args.next() {
                    Some(target) if chain.is_none() => chain = Some(target.clone()),
                    _ => return Err(
                        "prune takes one `--chain <ref>`, e.g. `--chain refs/stones/heads/old`."
                            .into(),
                    ),
                }
            }
            other => {
                return Err(format!(
                    "`prune` does not take `{other}`. See `git stone help`."
                ))
            }
        }
    }
    Ok(Command::Prune { apply, chain })
}

/// `git stone <id> <verb> ...`
fn parse_id_first(number: &str, rest: &[String]) -> Result<Command, String> {
    let Some((verb, tail)) = rest.split_first() else {
        return Err(format!(
            "Did you mean `git stone show {number}`? To create a stone named {number}, use `git stone -- {number}`."
        ));
    };
    let id = parse_id(number)?;
    match verb.as_str() {
        "diff" => Ok(Command::Diff {
            target: Target::Id(id),
            args: tail.to_vec(),
        }),
        "show" => no_args("show", tail).map(|()| Command::Show { id }),
        "commit" => Ok(Command::Commit {
            id,
            args: tail.to_vec(),
        }),
        "drop" => no_args("drop", tail).map(|()| Command::Drop { id }),
        other => Err(format!(
            "Unknown action `{other}` after stone {number}. Use diff, show, commit or drop."
        )),
    }
}

/// Everything else is a new stone. Flags may sit anywhere before `--`; the name is one argument.
fn parse_create(args: &[String]) -> Result<Command, String> {
    let (mut allow_empty, mut force, mut after_dashes) = (false, false, false);
    let (mut exclude, mut remember) = (Vec::new(), false);
    let mut description: Option<String> = None;
    let mut names: Vec<&str> = Vec::new();
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        if after_dashes {
            names.push(arg);
            continue;
        }
        match arg.as_str() {
            "--" => after_dashes = true,
            "--allow-empty" => allow_empty = true,
            "--force" => force = true,
            "--remember" => remember = true,
            "-x" | "--exclude" => match args.next() {
                Some(pattern) => exclude.push(pattern.clone()),
                None => return Err(format!("`{arg}` needs a pattern: git stone \"name\" -x '*.log'")),
            },
            s if s.starts_with("--exclude=") => exclude.push(s["--exclude=".len()..].to_string()),
            "-d" | "--description" => {
                // The value is taken as is, even if it starts with a dash.
                let Some(text) = args.next() else {
                    return Err(format!("`{arg}` needs the description text: git stone \"name\" -d \"what it is for\""));
                };
                if description.replace(text.clone()).is_some() {
                    return Err("The description was given twice. Use one -d.".into());
                }
            }
            s if s.starts_with("--description=") => {
                let text = s["--description=".len()..].to_string();
                if description.replace(text).is_some() {
                    return Err("The description was given twice. Use one -d.".into());
                }
            }
            s if s.starts_with('-') => {
                return Err(format!(
                    "Unknown option `{s}`. To create a stone whose name starts with a dash, use `git stone -- {s}`. See `git stone help`."
                ))
            }
            s if VERBS.contains(&s) => {
                return Err(format!(
                    "`{s}` is a command. To create a stone named {s}, use `git stone -- {s}`."
                ))
            }
            s if is_number(s) => {
                return Err(format!(
                    "`{s}` looks like a stone number. To create a stone named {s}, use `git stone -- {s}`."
                ))
            }
            s => names.push(s),
        }
    }
    if remember && exclude.is_empty() {
        return Err(
            "--remember saves what you pass with -x: git stone \"name\" -x '*.log' --remember"
                .into(),
        );
    }
    match names.as_slice() {
        [name] => Ok(Command::Create {
            name: (*name).to_string(),
            description,
            allow_empty,
            force,
            exclude,
            remember,
        }),
        [] => Err("A stone needs a name: git stone \"what you finished\"".into()),
        _ => Err(
            "A stone name must be one argument. Put it in quotes: git stone \"rate limit\"".into(),
        ),
    }
}

fn is_number(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

fn parse_id(s: &str) -> Result<u64, String> {
    if is_number(s) {
        s.parse()
            .map_err(|_| format!("`{s}` is too large to be a stone number"))
    } else {
        Err(format!("`{s}` is not a stone number"))
    }
}

fn one_id(verb: &str, rest: &[String]) -> Result<u64, String> {
    match rest {
        [id] => parse_id(id),
        _ => Err(format!(
            "{verb} needs one stone number: git stone {verb} <id>"
        )),
    }
}

fn no_args(verb: &str, rest: &[String]) -> Result<(), String> {
    match rest.first() {
        None => Ok(()),
        Some(extra) => Err(format!(
            "`{verb}` does not take `{extra}`. See `git stone help`."
        )),
    }
}

/// A command whose only option is `name`.
fn flag(verb: &str, rest: &[String], name: &str) -> Result<bool, String> {
    match rest {
        [] => Ok(false),
        [only] if only == name => Ok(true),
        [other, ..] => Err(format!(
            "`{verb}` does not take `{other}`. See `git stone help`."
        )),
    }
}

fn run() -> i32 {
    let mut args = Vec::new();
    for arg in std::env::args_os().skip(1) {
        match arg.into_string() {
            Ok(arg) => args.push(arg),
            Err(arg) => {
                eprintln!("git-stone: argument {arg:?} is not valid UTF-8");
                return 1;
            }
        }
    }
    let command = match parse(&args) {
        Ok(command) => command,
        Err(message) => {
            eprintln!("git-stone: {message}");
            return 1;
        }
    };
    match command {
        Command::Help => {
            print!("{HELP}");
            0
        }
        Command::Agents => {
            print!("{AGENT_MEMORY}");
            0
        }
        Command::Setup { agent, global } => report(cmd::setup::run(agent, global)),
        Command::SetupUpdate => report(cmd::setup::update()),
        Command::GuardCheck { phase } => cmd::guard::check(&phase),
        Command::Version => {
            println!("git-stone {}", env!("CARGO_PKG_VERSION"));
            0
        }
        other => report(dispatch(&other)),
    }
}

/// Problems the user can fix are shown as they are; internal ones are marked.
fn report(result: cmd::CmdResult) -> i32 {
    match result {
        Ok(code) => code,
        Err(e) => {
            if e.code == 1 {
                eprintln!("{}", e.message);
            } else {
                eprintln!("git-stone: {}", e.message);
            }
            e.code
        }
    }
}

fn dispatch(command: &Command) -> cmd::CmdResult {
    use cmd::{adopt, commit, create, diff, drop, list, prune, repair, review, show, status};
    let repo = repo::Repo::discover()?;
    match command {
        Command::Create {
            name,
            description,
            allow_empty,
            force,
            exclude,
            remember,
        } => create::run(
            &repo,
            name,
            description.as_deref(),
            *allow_empty,
            *force,
            exclude,
            *remember,
        ),
        Command::Exclude(action) => cmd::exclude::run(&repo, action),
        Command::Guard(action) => cmd::guard::run(&repo, action),
        Command::Status { porcelain } => status::run(&repo, *porcelain),
        Command::List { porcelain } => list::run(&repo, *porcelain),
        Command::Review => review::run(&repo),
        Command::Diff { target, args } => diff::run(&repo, target, args),
        Command::Show { id } => show::run(&repo, *id),
        Command::Drop { id } => drop::run(&repo, *id),
        Command::DropAll => drop::run_all(&repo),
        Command::Commit { id, args } => commit::run(&repo, *id, args),
        Command::CommitAll { edit } => commit::run_all(&repo, *edit),
        Command::Repair { apply } => repair::run(&repo, *apply),
        Command::Adopt { source } => adopt::run(&repo, source),
        Command::Prune { apply, chain } => prune::run(&repo, *apply, chain.as_deref()),
        Command::Help
        | Command::Version
        | Command::Agents
        | Command::Setup { .. }
        | Command::SetupUpdate
        | Command::GuardCheck { .. } => {
            unreachable!("handled before dispatch")
        }
    }
}

fn main() {
    exit(run());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(args: &[&str]) -> Result<Command, String> {
        let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        parse(&args)
    }

    fn create(name: &str) -> Command {
        Command::Create {
            name: name.into(),
            description: None,
            allow_empty: false,
            force: false,
            exclude: vec![],
            remember: false,
        }
    }

    fn strings(args: &[&str]) -> Vec<String> {
        args.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn agents_is_a_command_and_the_docs_show_the_same_block() {
        assert_eq!(p(&["agents"]), Ok(Command::Agents));
        assert!(p(&["agents", "x"]).is_err());
        let docs = include_str!("../docs/ai-agents.md");
        assert!(
            docs.contains(AGENT_MEMORY),
            "docs/ai-agents.md must show docs/agent-memory.md"
        );
    }

    #[test]
    fn prune_takes_apply_and_one_chain() {
        let prune = |apply, chain: Option<&str>| {
            Ok(Command::Prune {
                apply,
                chain: chain.map(String::from),
            })
        };
        assert_eq!(p(&["prune"]), prune(false, None));
        assert_eq!(p(&["prune", "--apply"]), prune(true, None));
        assert_eq!(
            p(&["prune", "--chain", "refs/stones/heads/x", "--apply"]),
            prune(true, Some("refs/stones/heads/x"))
        );
        assert!(p(&["prune", "--chain"]).is_err());
        assert!(p(&["prune", "--chain", "a", "--chain", "b"]).is_err());
        assert!(p(&["prune", "extra"]).is_err());
        // A stone can still be named "prune" after `--`.
        assert_eq!(p(&["--", "prune"]), Ok(create("prune")));
    }

    #[test]
    fn no_arguments_is_status() {
        assert_eq!(p(&[]), Ok(Command::Status { porcelain: false }));
    }

    #[test]
    fn name_with_spaces_is_one_argument() {
        assert_eq!(p(&["rate limit"]), Ok(create("rate limit")));
        assert_eq!(p(&["  padded  "]), Ok(create("  padded  ")));
    }

    #[test]
    fn several_name_arguments_are_refused() {
        assert!(p(&["rate", "limit"]).unwrap_err().contains("quotes"));
    }

    #[test]
    fn missing_name_is_refused() {
        assert!(p(&["--force"]).unwrap_err().contains("needs a name"));
        assert!(p(&["--"]).unwrap_err().contains("needs a name"));
    }

    #[test]
    fn dashes_allow_odd_names() {
        assert_eq!(p(&["--", "-fix"]), Ok(create("-fix")));
        assert_eq!(p(&["--", "42"]), Ok(create("42")));
        assert_eq!(p(&["--", "list"]), Ok(create("list")));
        assert_eq!(p(&["--", "--force"]), Ok(create("--force")));
    }

    #[test]
    fn leading_dash_without_dashes_is_refused() {
        let err = p(&["-fix"]).unwrap_err();
        assert!(err.contains("Unknown option `-fix`"));
        assert!(err.contains("git stone -- -fix"));
    }

    #[test]
    fn create_flags_work_before_or_after_the_name() {
        let expected = Command::Create {
            name: "auth".into(),
            description: None,
            allow_empty: true,
            force: true,
            exclude: vec![],
            remember: false,
        };
        assert_eq!(p(&["--allow-empty", "--force", "auth"]), Ok(expected));
        assert_eq!(
            p(&["auth", "--force"]),
            Ok(Command::Create {
                name: "auth".into(),
                description: None,
                allow_empty: false,
                force: true,
                exclude: vec![],
                remember: false,
            })
        );
        // After `--` a flag-looking word is part of the name.
        assert_eq!(
            p(&["--force", "--", "--allow-empty"]),
            Ok(Command::Create {
                name: "--allow-empty".into(),
                description: None,
                allow_empty: false,
                force: true,
                exclude: vec![],
                remember: false,
            })
        );
    }

    fn create_with(name: &str, description: &str) -> Command {
        Command::Create {
            name: name.into(),
            description: Some(description.into()),
            allow_empty: false,
            force: false,
            exclude: vec![],
            remember: false,
        }
    }

    #[test]
    fn description_flag() {
        let want = create_with("auth", "Adds JWT login.\nCovers /login.");
        let text = "Adds JWT login.\nCovers /login.";
        assert_eq!(p(&["auth", "-d", text]), Ok(want));
        assert_eq!(p(&["-d", text, "auth"]), Ok(create_with("auth", text)));
        assert_eq!(
            p(&["auth", "--description", text]),
            Ok(create_with("auth", text))
        );
        assert_eq!(
            p(&["auth", &format!("--description={text}")]),
            Ok(create_with("auth", text))
        );
        // The value may start with a dash, or be empty (the create command decides what that means).
        assert_eq!(p(&["auth", "-d", "-x"]), Ok(create_with("auth", "-x")));
        assert_eq!(p(&["auth", "-d", ""]), Ok(create_with("auth", "")));
        assert_eq!(p(&["auth", "--description="]), Ok(create_with("auth", "")));
        // -d goes before `--`; after it, everything is the name.
        assert_eq!(
            p(&["-d", "why", "--", "-fix"]),
            Ok(create_with("-fix", "why"))
        );
        assert!(p(&["--", "-fix", "-d", "why"]).is_err());
    }

    #[test]
    fn description_flag_mistakes() {
        assert!(p(&["auth", "-d"])
            .unwrap_err()
            .contains("needs the description"));
        assert!(p(&["auth", "-d", "a", "-d", "b"])
            .unwrap_err()
            .contains("twice"));
        assert!(p(&["auth", "-d", "a", "--description=b"])
            .unwrap_err()
            .contains("twice"));
        // The description text is not mistaken for the name.
        assert!(p(&["-d", "why"]).unwrap_err().contains("needs a name"));
        assert!(p(&["auth", "-d", "why", "extra"])
            .unwrap_err()
            .contains("quotes"));
    }

    #[test]
    fn lone_number_is_ambiguous() {
        let err = p(&["42"]).unwrap_err();
        assert_eq!(
            err,
            "Did you mean `git stone show 42`? To create a stone named 42, use `git stone -- 42`."
        );
        // Also when it sits behind a create flag.
        assert!(p(&["--force", "42"])
            .unwrap_err()
            .contains("git stone -- 42"));
        // Too big for an id still gets the same hint.
        assert!(p(&["99999999999999999999999"])
            .unwrap_err()
            .contains("Did you mean"));
    }

    #[test]
    fn verb_as_name_needs_dashes() {
        assert!(p(&["--force", "status"])
            .unwrap_err()
            .contains("git stone -- status"));
    }

    #[test]
    fn id_then_verb() {
        assert_eq!(p(&["3", "show"]), Ok(Command::Show { id: 3 }));
        assert_eq!(p(&["3", "drop"]), Ok(Command::Drop { id: 3 }));
        assert_eq!(
            p(&["3", "diff", "--stat", "--", "src"]),
            Ok(Command::Diff {
                target: Target::Id(3),
                args: strings(&["--stat", "--", "src"]),
            })
        );
        assert_eq!(
            p(&["1", "commit", "-m", "feat: add auth"]),
            Ok(Command::Commit {
                id: 1,
                args: strings(&["-m", "feat: add auth"]),
            })
        );
    }

    #[test]
    fn verb_then_id() {
        assert_eq!(p(&["show", "3"]), Ok(Command::Show { id: 3 }));
        assert_eq!(p(&["drop", "3"]), Ok(Command::Drop { id: 3 }));
        assert_eq!(
            p(&["diff", "2", "--stat"]),
            Ok(Command::Diff {
                target: Target::Id(2),
                args: strings(&["--stat"]),
            })
        );
        assert_eq!(
            p(&["diff", "current"]),
            Ok(Command::Diff {
                target: Target::Current,
                args: vec![],
            })
        );
        assert_eq!(
            p(&["commit", "1", "--no-verify"]),
            Ok(Command::Commit {
                id: 1,
                args: strings(&["--no-verify"]),
            })
        );
    }

    #[test]
    fn unknown_verb_after_an_id() {
        let err = p(&["3", "frobnicate"]).unwrap_err();
        assert!(err.contains("Unknown action `frobnicate` after stone 3"));
        assert!(p(&["3", "show", "extra"]).is_err());
        assert!(p(&["3", "drop", "extra"]).is_err());
    }

    #[test]
    fn bad_ids_are_refused() {
        assert!(p(&["show", "abc"])
            .unwrap_err()
            .contains("not a stone number"));
        assert!(p(&["drop"]).is_err());
        assert!(p(&["drop", "1", "2"]).is_err());
        assert!(p(&["show", "99999999999999999999999"])
            .unwrap_err()
            .contains("too large"));
        assert!(p(&["diff"]).is_err());
        assert!(p(&["commit"]).is_err());
    }

    #[test]
    fn read_commands_and_flags() {
        assert_eq!(p(&["status"]), Ok(Command::Status { porcelain: false }));
        assert_eq!(
            p(&["status", "--porcelain"]),
            Ok(Command::Status { porcelain: true })
        );
        assert_eq!(
            p(&["list", "--porcelain"]),
            Ok(Command::List { porcelain: true })
        );
        assert_eq!(p(&["review"]), Ok(Command::Review));
        assert!(p(&["status", "--nope"]).is_err());
        assert!(p(&["list", "--porcelain", "extra"]).is_err());
        assert!(p(&["review", "extra"]).is_err());
    }

    #[test]
    fn setup_update() {
        assert_eq!(p(&["setup", "--update"]), Ok(Command::SetupUpdate));
        assert!(p(&["setup", "--update", "--global"]).is_err());
        assert!(p(&["setup", "--update", "--agent", "claude"]).is_err());
    }

    #[test]
    fn drop_all() {
        assert_eq!(p(&["drop", "all"]), Ok(Command::DropAll));
        assert_eq!(p(&["drop", "2"]), Ok(Command::Drop { id: 2 }));
        assert!(p(&["drop", "all", "2"]).is_err());
        assert!(p(&["drop"]).is_err());
    }

    #[test]
    fn guard_commands() {
        assert_eq!(p(&["guard"]), Ok(Command::Guard(GuardAction::Show)));
        assert_eq!(
            p(&["guard", "block"]),
            Ok(Command::Guard(GuardAction::Block))
        );
        assert_eq!(p(&["guard", "warn"]), Ok(Command::Guard(GuardAction::Warn)));
        assert_eq!(p(&["guard", "off"]), Ok(Command::Guard(GuardAction::Off)));
        assert!(p(&["guard", "maybe"]).is_err());
        assert!(p(&["guard", "block", "off"]).is_err());
        assert_eq!(
            p(&["guard-check", "prepared"]),
            Ok(Command::GuardCheck {
                phase: "prepared".into()
            })
        );
        assert!(p(&["guard-check"]).is_err());
        // Both words are commands, so a stone with that name needs `--`.
        assert!(p(&["--", "guard"]).is_ok());
    }

    #[test]
    fn exclude_flags_on_create() {
        assert_eq!(
            p(&["auth", "-x", "*.log", "--exclude=tmp/", "--remember"]),
            Ok(Command::Create {
                name: "auth".into(),
                description: None,
                allow_empty: false,
                force: false,
                exclude: vec!["*.log".into(), "tmp/".into()],
                remember: true,
            })
        );
        assert!(p(&["auth", "-x"]).is_err());
        assert!(p(&["auth", "--remember"]).is_err());
    }

    #[test]
    fn exclude_command() {
        assert_eq!(p(&["exclude"]), Ok(Command::Exclude(ExcludeAction::List)));
        assert_eq!(
            p(&["exclude", "--remove", "*.log"]),
            Ok(Command::Exclude(ExcludeAction::Remove("*.log".into())))
        );
        assert_eq!(
            p(&["exclude", "--clear"]),
            Ok(Command::Exclude(ExcludeAction::Clear))
        );
        assert!(p(&["exclude", "--remove"]).is_err());
        assert!(p(&["exclude", "extra"]).is_err());
        // A stone named like the command needs `--`.
        assert!(p(&["--", "exclude"]).is_ok());
    }

    #[test]
    fn commit_all_repair_adopt_help_version() {
        assert_eq!(p(&["commit-all"]), Ok(Command::CommitAll { edit: false }));
        assert_eq!(
            p(&["commit-all", "--edit"]),
            Ok(Command::CommitAll { edit: true })
        );
        assert_eq!(p(&["repair"]), Ok(Command::Repair { apply: false }));
        assert_eq!(
            p(&["repair", "--apply"]),
            Ok(Command::Repair { apply: true })
        );
        assert_eq!(
            p(&["adopt", "detached"]),
            Ok(Command::Adopt {
                source: "detached".into()
            })
        );
        assert!(p(&["adopt"]).is_err());
        assert!(p(&["adopt", "a", "b"]).is_err());
        assert_eq!(p(&["help"]), Ok(Command::Help));
        assert_eq!(p(&["version"]), Ok(Command::Version));
    }
}
