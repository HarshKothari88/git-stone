mod common;

use std::fs;

use common::Repo;

#[test]
fn project_setup_creates_the_agent_files_with_the_block() {
    let r = Repo::new();
    let out = r.ok(&["setup", "--agent", "both"]);
    println!("{out}");
    assert!(
        out.contains("Claude Code: created") && out.contains("Codex: created"),
        "{out}"
    );
    for file in ["CLAUDE.md", "AGENTS.md"] {
        let text = r.read(file);
        assert!(text.starts_with("<!-- git-stone:begin"), "{file}: {text}");
        assert!(text.contains("## Git Stone (task boundaries)"));
        assert!(text.trim_end().ends_with("<!-- git-stone:end -->"));
    }
    // Only the project's files were written, and nothing else in the repository.
    assert!(!r.home().join(".claude").exists() && !r.home().join(".codex").exists());
    // `agents` prints the same block that setup wrote.
    let block = r.ok(&["agents"]);
    assert!(r.read("CLAUDE.md").contains(block.trim_end()));
}

#[test]
fn project_setup_works_from_a_subdirectory_and_writes_at_the_top() {
    let r = Repo::new();
    fs::create_dir(r.dir.join("src")).unwrap();
    let o = r.stone_in(&r.dir.join("src"), &["setup", "--agent", "claude"]);
    assert_eq!(o.code, 0, "{}", o.stderr);
    assert!(r.dir.join("CLAUDE.md").exists() && !r.dir.join("src/CLAUDE.md").exists());
    assert!(!r.dir.join("AGENTS.md").exists());
}

#[test]
fn existing_instructions_are_kept_exactly_and_a_second_run_changes_nothing() {
    let r = Repo::new();
    let mine = "# My rules\n\n- be terse\n- no emojis";
    r.write("CLAUDE.md", mine);
    let first = r.ok(&["setup", "--agent", "claude"]);
    assert!(first.contains("updated"), "{first}");
    let after = r.read("CLAUDE.md");
    assert!(after.starts_with(mine), "{after}");
    assert_eq!(after.matches("git-stone:begin").count(), 1);

    let second = r.ok(&["setup", "--agent", "claude"]);
    assert!(second.contains("already up to date"), "{second}");
    assert_eq!(r.read("CLAUDE.md"), after);
}

#[test]
fn an_older_block_is_refreshed_in_place_and_the_text_around_it_stays() {
    let r = Repo::new();
    r.write(
        "AGENTS.md",
        "top\n\n<!-- git-stone:begin (old) -->\nOLD INSTRUCTIONS\n<!-- git-stone:end -->\n\nbottom\n",
    );
    r.ok(&["setup", "--agent", "codex"]);
    let text = r.read("AGENTS.md");
    assert!(
        text.starts_with("top\n\n") && text.ends_with("\n\nbottom\n"),
        "{text}"
    );
    assert!(!text.contains("OLD INSTRUCTIONS"));
    assert!(text.contains("## Git Stone (task boundaries)"));
}

#[test]
fn a_damaged_block_changes_no_file_at_all() {
    let r = Repo::new();
    r.write("AGENTS.md", "<!-- git-stone:begin -->\nno end line\n");
    let o = r.stone(&["setup", "--agent", "both"]);
    assert_eq!(o.code, 1);
    assert!(
        o.stderr.contains("damaged Git Stone block") && o.stderr.contains("Nothing was changed"),
        "{}",
        o.stderr
    );
    // Claude's file would have been fine, but the run is all or nothing.
    assert!(!r.dir.join("CLAUDE.md").exists());
    assert_eq!(
        r.read("AGENTS.md"),
        "<!-- git-stone:begin -->\nno end line\n"
    );
}

#[test]
fn a_symlinked_memory_file_stays_a_symlink() {
    let r = Repo::new();
    r.write("AGENTS.md", "shared rules\n");
    std::os::unix::fs::symlink("AGENTS.md", r.dir.join("CLAUDE.md")).unwrap();
    r.ok(&["setup", "--agent", "claude"]);
    assert!(fs::symlink_metadata(r.dir.join("CLAUDE.md"))
        .unwrap()
        .file_type()
        .is_symlink());
    assert!(r.read("AGENTS.md").starts_with("shared rules\n"));
    assert!(r.read("AGENTS.md").contains("git-stone:begin"));
}

#[test]
fn project_setup_outside_a_repository_is_refused_but_global_works() {
    let r = Repo::new();
    let elsewhere = tempfile::tempdir().unwrap();
    let o = r.stone_in(elsewhere.path(), &["setup", "--agent", "claude"]);
    assert_eq!(o.code, 1);
    assert!(o.stderr.contains("add --global"), "{}", o.stderr);
    assert!(fs::read_dir(elsewhere.path()).unwrap().next().is_none());

    let o = r.stone_in(
        elsewhere.path(),
        &["setup", "--agent", "claude", "--global"],
    );
    assert_eq!(o.code, 0, "{}", o.stderr);
    assert!(r.home().join(".claude/CLAUDE.md").exists());
}

#[test]
fn global_setup_writes_the_agents_own_files_and_keeps_existing_ones() {
    let r = Repo::new();
    let claude = r.home().join(".claude");
    fs::create_dir_all(&claude).unwrap();
    fs::write(claude.join("CLAUDE.md"), "# Global rules\nkeep me\n").unwrap();
    let out = r.ok(&["setup", "--agent", "both", "--global"]);
    println!("{out}");
    let text = fs::read_to_string(claude.join("CLAUDE.md")).unwrap();
    assert!(text.starts_with("# Global rules\nkeep me\n"), "{text}");
    assert!(text.contains("git-stone:begin"));
    assert!(fs::read_to_string(r.home().join(".codex/AGENTS.md"))
        .unwrap()
        .contains("git-stone:begin"));
    // Nothing was written into the project.
    assert!(!r.dir.join("CLAUDE.md").exists() && !r.dir.join("AGENTS.md").exists());
}

#[test]
fn the_agents_own_config_folder_variables_are_honoured() {
    let r = Repo::new();
    let (claude, codex) = (r.home().join("c-dir"), r.home().join("x-dir"));
    let o = r.stone_env(
        &r.dir,
        &["setup", "--agent", "both", "--global"],
        &[
            ("CLAUDE_CONFIG_DIR", claude.to_str().unwrap()),
            ("CODEX_HOME", codex.to_str().unwrap()),
        ],
    );
    assert_eq!(o.code, 0, "{}", o.stderr);
    assert!(claude.join("CLAUDE.md").exists() && codex.join("AGENTS.md").exists());
    assert!(!r.home().join(".claude").exists());
}

#[test]
fn bad_arguments_are_refused() {
    let r = Repo::new();
    for args in [
        &["setup"][..],
        &["setup", "--global"],
        &["setup", "--agent"],
        &["setup", "--agent", "gemini"],
        &["setup", "--agent", "claude", "extra"],
    ] {
        let o = r.stone(args);
        assert_eq!(o.code, 1, "{args:?}: {}{}", o.stdout, o.stderr);
    }
    assert!(!r.dir.join("CLAUDE.md").exists());
}
