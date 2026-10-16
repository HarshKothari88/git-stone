mod common;

use common::Repo;

/// The first line of `git stone <name>` output: `Stone 3 created: name`.
fn created_id(output: &str) -> u64 {
    output
        .strip_prefix("Stone ")
        .and_then(|rest| rest.split(' ').next())
        .and_then(|n| n.parse().ok())
        .unwrap_or_else(|| panic!("unexpected create output: {output}"))
}

#[test]
fn three_stones_in_a_row_with_a_dependency_chain() {
    let r = Repo::new();
    r.write("auth.txt", "login\n");
    assert_eq!(created_id(&r.ok(&["auth"])), 1);
    // Stone 2 edits the file Stone 1 created.
    r.write("auth.txt", "login\nrefresh\n");
    assert_eq!(created_id(&r.ok(&["authorization"])), 2);
    r.write("limit.txt", "100/min\n");
    let third = r.ok(&["rate-limit"]);
    assert_eq!(created_id(&third), 3);
    assert!(third.contains("1 file changed, +1 -0"), "{third}");

    let anchor_tree = r.git(&["rev-parse", "HEAD^{tree}"]).trim().to_string();
    let trees: Vec<String> = (1..=3).map(|i| r.stone_tree("main", i)).collect();
    let bases = [anchor_tree, trees[0].clone(), trees[1].clone()];
    for i in 0..3 {
        let expected = r.git(&["diff", &bases[i], &trees[i]]);
        let shown = r.stone(&["diff", &(i + 1).to_string()]);
        assert_eq!(shown.code, 0, "{}", shown.stderr);
        assert_eq!(shown.stdout, expected, "diff of Stone {}", i + 1);
        // Both spellings.
        let other = r.stone(&[&(i + 1).to_string(), "diff"]);
        assert_eq!(other.stdout, expected);
    }
    // Stone 2 only holds its own edit, not the file Stone 1 made.
    let second = r.stone(&["diff", "2"]).stdout;
    assert!(
        second.contains("+refresh") && !second.contains("+login"),
        "{second}"
    );

    assert_eq!(
        r.ok(&["list"]),
        "  1  auth\n  2  authorization\n  3  rate-limit\n"
    );
}

#[test]
fn diff_passes_options_and_paths_to_git() {
    let r = Repo::new();
    r.write("a.txt", "a\n");
    r.write("b.txt", "b\n");
    r.ok(&["both"]);
    let stat = r.stone(&["diff", "1", "--stat"]);
    assert!(
        stat.stdout.contains("a.txt") && stat.stdout.contains("2 files changed"),
        "{}",
        stat.stdout
    );
    let one = r.stone(&["diff", "1", "--", "a.txt"]);
    assert!(one.stdout.contains("a.txt") && !one.stdout.contains("b.txt"));
    // git's own exit code comes through.
    let quiet = r.stone(&["diff", "1", "--exit-code", "--quiet"]);
    assert_eq!(quiet.code, 1);
}

#[test]
fn diff_current_is_the_unstoned_work() {
    let r = Repo::new();
    r.write("one.txt", "1\n");
    r.ok(&["one"]);
    r.write("two.txt", "2\n");
    let d = r.stone(&["diff", "current"]);
    assert!(
        d.stdout.contains("two.txt") && !d.stdout.contains("one.txt"),
        "{}",
        d.stdout
    );
    assert_eq!(
        r.stone(&["diff", "current", "--quiet", "--exit-code"]).code,
        1
    );
}

#[test]
fn create_read_commands_and_drop_in_never_touch_the_repo() {
    let r = Repo::new();
    r.write("tracked-change.txt", "x\n");
    r.git(&["add", "tracked-change.txt"]); // a staged change must stay staged
    r.write("untracked.txt", "u\n");
    r.write(".gitignore", "ignored.log\n");
    r.write("ignored.log", "noise\n");

    let before = r.fingerprint();
    r.ok(&["first", "-d", "details"]);
    r.assert_untouched(&before);
    r.write("later.txt", "l\n");
    let before = r.fingerprint();
    r.ok(&["second"]);
    r.assert_untouched(&before);

    for args in [
        vec!["status"],
        vec![],
        vec!["status", "--porcelain"],
        vec!["list"],
        vec!["list", "--porcelain"],
        vec!["review"],
        vec!["show", "1"],
        vec!["1", "show"],
        vec!["diff", "1"],
        vec!["diff", "2", "--stat"],
        vec!["diff", "current"],
    ] {
        let before = r.fingerprint();
        let o = r.stone(&args);
        assert_eq!(o.code, 0, "{args:?}: {}", o.stderr);
        r.assert_untouched(&before);
    }
    // The staged change is still staged, the ignored file still absent from every stone.
    assert!(r
        .git(&["diff", "--cached", "--name-only"])
        .contains("tracked-change.txt"));
    let tree = r.stone_tree("main", 2);
    assert!(!r
        .git(&["ls-tree", "-r", "--name-only", &tree])
        .contains("ignored.log"));
}

#[test]
fn state_is_a_single_ref_and_the_branch_never_moves() {
    let r = Repo::new();
    let head = r.git(&["rev-parse", "HEAD"]);
    r.write("a.txt", "a\n");
    r.ok(&["a"]);
    r.write("b.txt", "b\n");
    r.ok(&["b"]);
    assert_eq!(r.git(&["rev-parse", "HEAD"]), head);
    let refs = r.git(&["for-each-ref", "--format=%(refname)", "refs/stones/"]);
    assert_eq!(refs, "refs/stones/heads/main\n");
    // The anchor is the only parent of the state commit.
    let parents = r.git(&["rev-list", "--parents", "-n1", "refs/stones/heads/main"]);
    assert_eq!(
        parents.trim(),
        format!(
            "{} {}",
            r.git(&["rev-parse", "refs/stones/heads/main"]).trim(),
            head.trim()
        )
    );
    r.git(&["fsck", "--no-dangling"]);
}

#[test]
fn stones_on_a_detached_head_are_per_worktree() {
    let r = Repo::new();
    r.git(&["checkout", "-q", "--detach"]);
    r.write("d.txt", "d\n");
    r.ok(&["on detached"]);
    assert!(r.state_ref_exists("refs/worktree/stones/detached"));
    assert_eq!(r.ok(&["list"]), "  1  on detached\n");
    let status = r.ok(&["status"]);
    assert!(status.starts_with("Detached HEAD at "), "{status}");
    assert!(status.contains("stones stay with this worktree"));

    // A second worktree, also detached, has its own chain.
    let other = r.tmp.path().join("other");
    r.git(&["worktree", "add", "-q", "--detach", other.to_str().unwrap()]);
    assert!(r
        .stone_in(&other, &["list"])
        .stdout
        .contains("No stones yet."));
    std::fs::write(other.join("o.txt"), "o\n").unwrap();
    assert_eq!(r.stone_in(&other, &["other work"]).code, 0);
    assert_eq!(r.stone_in(&other, &["list"]).stdout, "  1  other work\n");
    assert_eq!(r.ok(&["list"]), "  1  on detached\n");

    // Going back to the branch does not show detached stones, and returning finds them.
    r.git(&["checkout", "-q", "main"]);
    assert!(r.ok(&["list"]).contains("No stones yet."));
}

#[test]
fn stones_on_an_unborn_branch() {
    let r = Repo::unborn();
    r.write("first.txt", "1\n");
    r.write("dir/second.txt", "2\n");
    let before = r.fingerprint();
    let created = r.ok(&["start"]);
    assert!(created.contains("2 files changed, +2 -0"), "{created}");
    r.assert_untouched(&before);

    // Stone 1's diff is every file against the empty tree.
    let empty = r
        .git(&["hash-object", "-t", "tree", "--stdin"])
        .trim()
        .to_string();
    let _ = empty;
    let diff = r.ok(&["diff", "1"]);
    assert!(
        diff.contains("+++ b/first.txt") && diff.contains("+++ b/dir/second.txt"),
        "{diff}"
    );
    let status = r.ok(&["status"]);
    assert!(
        status.starts_with("Branch: main (no commits yet)"),
        "{status}"
    );
    assert!(
        status.contains("Current changes since Stone 1:") && status.contains("clean"),
        "{status}"
    );
    r.write("third.txt", "3\n");
    r.ok(&["next"]);
    assert_eq!(r.ok(&["list"]), "  1  start\n  2  next\n");
    assert!(r
        .ok(&["show", "1"])
        .contains("Base: the start (no commits yet)"));
}

#[test]
fn every_branch_keeps_its_own_chain_across_switches() {
    let r = Repo::new();
    r.write("a.txt", "a\n");
    r.ok(&["on main"]);
    // Park the uncommitted file the way a user would, then work on another branch.
    r.git(&["checkout", "-q", "-b", "other"]);
    assert!(
        r.ok(&["list"]).contains("No stones yet."),
        "other must not see main's stones"
    );
    r.write("o.txt", "o\n");
    r.ok(&["on other"]);
    assert_eq!(r.ok(&["list"]), "  1  on other\n");

    r.git(&["checkout", "-q", "main"]);
    assert_eq!(r.ok(&["list"]), "  1  on main\n");
    r.git(&["checkout", "-q", "other"]);
    assert_eq!(r.ok(&["list"]), "  1  on other\n");
    // Ids are per branch.
    r.write("o2.txt", "o\n");
    assert!(r.ok(&["more on other"]).starts_with("Stone 2 created"));
    r.git(&["checkout", "-q", "main"]);
    r.write("a2.txt", "a\n");
    assert!(r.ok(&["more on main"]).starts_with("Stone 2 created"));
}

#[test]
fn hostile_names_are_stored_and_printed_exactly() {
    let r = Repo::new();
    let names = [
        "$(touch pwned)",
        "a;b && c",
        "-rf",
        "émoji 🪨 name",
        "back\\slash 'q' \"d\"",
        "  padded  ",
    ];
    for (i, name) in names.iter().enumerate() {
        r.write(&format!("f{i}.txt"), "x\n");
        let args: Vec<&str> = if name.trim().starts_with('-') {
            vec!["--", name]
        } else {
            vec![name]
        };
        let o = r.stone(&args);
        assert_eq!(o.code, 0, "{name}: {}", o.stderr);
        assert!(
            o.stdout
                .starts_with(&format!("Stone {} created: {}\n", i + 1, name.trim())),
            "{}",
            o.stdout
        );
    }
    assert!(!r.dir.join("pwned").exists());
    let list = r.ok(&["list"]);
    for name in names {
        assert!(list.contains(&format!("  {}\n", name.trim())), "{list}");
    }
    let meta = r.git(&["cat-file", "-p", "refs/stones/heads/main:meta"]);
    assert!(
        meta.contains("stone 1 ") && meta.contains(" $(touch pwned)\n"),
        "{meta}"
    );
}

#[test]
fn a_name_that_looks_like_a_command_or_number_works_after_dashes() {
    let r = Repo::new();
    r.write("a.txt", "a\n");
    assert!(r.ok(&["--", "list"]).starts_with("Stone 1 created: list"));
    r.write("b.txt", "b\n");
    assert!(r.ok(&["--", "42"]).starts_with("Stone 2 created: 42"));
    assert_eq!(r.ok(&["list"]), "  1  list\n  2  42\n");
}

#[test]
fn descriptions_show_up_in_create_show_review_and_never_carry_escapes() {
    let r = Repo::new();
    r.write("auth.txt", "a\n");
    let created = r.ok(&["auth", "-d", "Adds JWT login.\nCovers /login.\x1b[31m"]);
    assert!(
        created.contains("  Adds JWT login.\n  Covers /login.[31m"),
        "{created:?}"
    );
    assert!(!created.contains('\x1b'));
    r.write("plain.txt", "p\n");
    r.ok(&["plain"]);

    let show = r.ok(&["show", "1"]);
    assert!(show.starts_with("Stone 1: auth\nCreated: "), "{show}");
    assert!(show.contains("Base: branch commit "));
    assert!(
        show.contains("\n  Adds JWT login.\n  Covers /login.[31m\n"),
        "{show:?}"
    );
    assert!(show.contains("auth.txt") && show.contains("+a"));
    assert!(!show.contains('\x1b'));
    assert!(r.ok(&["show", "2"]).contains("Base: Stone 1"));

    let review = r.ok(&["review"]);
    assert!(review.contains("Stone 1  auth\n    Adds JWT login.\n    Covers /login.[31m\n    1 file changed, +1 -0\n"), "{review}");
    assert!(review.contains("Stone 2  plain\n    1 file changed, +1 -0\n"));
    assert!(review.ends_with("CURRENT\n    no changes\n"), "{review}");

    // The raw text is kept as typed, in its own blob.
    let raw = r.git(&["cat-file", "-p", "refs/stones/heads/main:desc/1"]);
    assert!(raw.contains('\x1b'));
    // Whitespace-only means no description.
    r.write("c.txt", "c\n");
    let c = r.ok(&["third", "-d", "   \n  "]);
    assert_eq!(c.lines().count(), 2, "{c}");
    assert!(!r.state_ref_exists("refs/stones/heads/main:desc/3"));
}

#[test]
fn status_shows_stones_and_current_changes() {
    let r = Repo::new();
    r.write("src/auth.ts", "a\n");
    r.ok(&["auth"]);
    r.write("src/rate-limit.ts", "r\n");
    r.ok(&["rate-limit"]);
    r.write("src/auth.ts", "a2\n");
    r.write("src/new.ts", "n\n");
    std::fs::remove_file(r.dir.join("base.txt")).unwrap();
    let status = r.ok(&["status"]);
    let expected = "Branch: main\n\nStones:\n  1  auth\n  2  rate-limit\n\nCurrent changes since Stone 2:\n  D base.txt\n  M src/auth.ts\n  A src/new.ts\n\nUnstoned changes: 3 files\n";
    assert_eq!(status, expected);
    assert_eq!(r.ok(&[]), status);
}

#[test]
fn status_with_no_stones_and_a_clean_tree() {
    let r = Repo::new();
    let status = r.ok(&["status"]);
    let head = r.git(&["rev-parse", "--short=7", "HEAD"]);
    assert_eq!(
        status,
        format!(
            "Branch: main\n\nStones:\n  none\n\nCurrent changes since branch commit {}:\n  clean\n",
            head.trim()
        )
    );
}

#[test]
fn status_caps_a_long_change_list() {
    let r = Repo::new();
    for i in 0..60 {
        r.write(&format!("f{i:02}.txt"), "x\n");
    }
    let status = r.ok(&["status"]);
    assert!(status.contains("  ... and 10 more\n"), "{status}");
    assert!(status.contains("Unstoned changes: 60 files"));
    assert_eq!(status.matches("  A f").count(), 50);
}

#[test]
fn status_warns_about_renamed_branch_orphans_and_detached_chains() {
    let r = Repo::new();
    r.write("a.txt", "a\n");
    r.ok(&["work"]);
    r.git(&["branch", "-m", "main", "renamed"]);
    let status = r.ok(&["status"]);
    assert!(
        status.contains("Stones exist for branch `main`, which is gone"),
        "{status}"
    );
    assert!(status.contains("git stone adopt main"));

    // Stones made on a detached HEAD, then a branch is made at the same commit.
    let r = Repo::new();
    r.git(&["checkout", "-q", "--detach"]);
    r.write("d.txt", "d\n");
    r.ok(&["detached work"]);
    r.git(&["checkout", "-q", "-b", "feature"]);
    let status = r.ok(&["status"]);
    assert!(status.contains("git stone adopt detached"), "{status}");
}

#[test]
fn status_warns_when_the_branch_moved_and_read_commands_still_work() {
    let r = Repo::new();
    r.write("a.txt", "a\n");
    r.ok(&["a"]);
    r.git(&["commit", "-q", "--allow-empty", "-m", "someone else"]);
    let status = r.ok(&["status"]);
    assert!(
        status.contains("Warnings:") && status.contains("moved since the stones were made"),
        "{status}"
    );
    let list = r.stone(&["list"]);
    assert_eq!(list.code, 0);
    assert_eq!(list.stdout, "  1  a\n");
    assert!(
        list.stderr.contains("moved since the stones were made"),
        "{}",
        list.stderr
    );
    assert_eq!(r.stone(&["show", "1"]).code, 0);
    assert_eq!(r.stone(&["review"]).code, 0);
}

#[test]
fn status_warns_in_two_worktrees_and_in_a_stash() {
    let r = Repo::new();
    r.write("a.txt", "a\n");
    r.ok(&["a"]);
    r.git(&["stash", "-u", "-q"]);
    let status = r.ok(&["status"]);
    assert!(
        status.contains("matches the branch commit, not Stone 1. Did you stash?"),
        "{status}"
    );
    r.git(&["stash", "pop", "-q"]);

    let other = r.tmp.path().join("twin");
    r.git(&[
        "worktree",
        "add",
        "-q",
        "--force",
        other.to_str().unwrap(),
        "main",
    ]);
    assert!(r
        .ok(&["status"])
        .contains("checked out in more than one worktree"));
}

#[test]
fn status_warns_about_a_dirty_submodule() {
    let r = Repo::new();
    let source = r.tmp.path().join("sub-src");
    std::fs::create_dir(&source).unwrap();
    r.git_in(&source, &["init", "-q", "-b", "main"]);
    std::fs::write(source.join("s.txt"), "s\n").unwrap();
    r.git_in(&source, &["add", "s.txt"]);
    r.git_in(&source, &["commit", "-q", "-m", "s"]);
    r.git(&[
        "-c",
        "protocol.file.allow=always",
        "submodule",
        "add",
        "-q",
        source.to_str().unwrap(),
        "sub",
    ]);
    r.git(&["commit", "-q", "-m", "add sub"]);
    assert!(!r.ok(&["status"]).contains("Warnings"));
    r.write("sub/dirty.txt", "d\n");
    let status = r.ok(&["status"]);
    assert!(
        status.contains("Submodule `sub` has uncommitted changes"),
        "{status}"
    );
}

#[test]
fn porcelain_output_is_versioned_and_nul_terminated() {
    let r = Repo::new();
    r.write("a.txt", "a\n");
    r.ok(&["first", "-d", "line one\nline\ttwo"]);
    r.write("b.txt", "b\n");

    let list = r.ok(&["list", "--porcelain"]);
    let records: Vec<&str> = list.split('\0').collect();
    assert_eq!(records[0], "git-stone-porcelain\t1\tlist");
    let tree = r.stone_tree("main", 1);
    let fields: Vec<&str> = records[1].splitn(6, '\t').collect();
    assert_eq!(fields[0], "stone");
    assert_eq!(fields[1], "1");
    assert!(fields[2].parse::<u64>().is_ok());
    assert_eq!(fields[3], tree);
    assert_eq!(fields[4], "first");
    assert_eq!(fields[5], "line one\nline\ttwo");
    assert_eq!(
        records.len(),
        3,
        "header, one stone, and the empty tail: {records:?}"
    );
    assert_eq!(records[2], "");
    assert!(
        !list.contains('\n') || list.contains("line one\n"),
        "no stray newlines between records"
    );

    let status = r.ok(&["status", "--porcelain"]);
    let records: Vec<&str> = status.split('\0').filter(|r| !r.is_empty()).collect();
    assert_eq!(records[0], "git-stone-porcelain\t1\tstatus");
    assert_eq!(records[1], "scope\trefs/heads/main");
    assert!(records[2].starts_with("head\t") && records[3].starts_with("anchor\t"));
    assert!(records.iter().any(|r| r.starts_with("stone\t1\t")));
    assert!(records.contains(&"change\tA\tb.txt"));
    assert_eq!(r.ok(&["list", "--porcelain"]).split('\0').count(), 3);

    // No stones: header only.
    let empty = Repo::new();
    assert_eq!(
        empty.ok(&["list", "--porcelain"]),
        "git-stone-porcelain\t1\tlist\0"
    );
}

#[test]
fn create_failpoint_leaves_no_state() {
    let r = Repo::new();
    r.write("a.txt", "a\n");
    let before = r.fingerprint();
    let o = r.stone_env(
        &r.dir,
        &["doomed"],
        &[("GIT_STONE_FAILPOINT", "create:before-cas")],
    );
    assert_eq!(o.code, 99, "{}{}", o.stdout, o.stderr);
    assert!(!r.state_ref_exists("refs/stones/heads/main"));
    r.assert_untouched(&before);
    // Objects written before the exit are harmless garbage; the next run works normally.
    assert!(r.ok(&["fine"]).starts_with("Stone 1 created: fine"));
}

#[test]
fn creating_with_zero_stones_re_anchors_to_the_new_head() {
    let r = Repo::new();
    r.write("a.txt", "a\n");
    r.ok(&["a"]);
    // A branch with no stones yet starts its chain wherever HEAD is now.
    r.git(&["checkout", "-q", "-b", "fresh"]);
    r.git(&["commit", "-q", "--allow-empty", "-m", "moves head"]);
    r.write("z.txt", "z\n");
    assert!(r.ok(&["z"]).starts_with("Stone 1 created"));
    let meta = r.git(&["cat-file", "-p", "refs/stones/heads/fresh:meta"]);
    let head = r.git(&["rev-parse", "HEAD"]);
    assert!(meta.contains(&format!("anchor {}", head.trim())), "{meta}");
}
