mod common;

use common::Repo;

fn state_ref(r: &Repo) -> String {
    r.git(&["rev-parse", "refs/stones/heads/main"])
        .trim()
        .to_string()
}

fn head_tree(r: &Repo) -> String {
    r.git(&["rev-parse", "HEAD^{tree}"]).trim().to_string()
}

fn anchor(r: &Repo) -> String {
    r.git(&["cat-file", "-p", "refs/stones/heads/main:meta"])
        .lines()
        .find_map(|l| l.strip_prefix("anchor "))
        .unwrap()
        .to_string()
}

/// What stone `id` changes compared with `base` (a tree).
fn changes(r: &Repo, base: &str, id: u64) -> String {
    r.git(&[
        "diff-tree",
        "-r",
        "--name-status",
        base,
        &r.stone_tree("main", id),
    ])
}

/// Two stones that add `a.txt` and `b.txt`, then an ordinary commit of an unrelated `other.txt`.
fn moved_by_an_unrelated_commit() -> Repo {
    let r = Repo::new();
    r.write("a.txt", "a\n");
    r.ok(&["one"]);
    r.write("b.txt", "b\n");
    r.ok(&["two"]);
    r.write("other.txt", "other\n");
    r.git(&["add", "other.txt"]);
    r.git(&["commit", "-q", "-m", "unrelated"]);
    r
}

#[test]
fn an_unrelated_commit_blocks_changes_until_repair_is_applied() {
    let r = moved_by_an_unrelated_commit();
    let old_base = r.git(&["rev-parse", "HEAD~1^{tree}"]).trim().to_string();
    let old_diffs = (changes(&r, &old_base, 1), {
        let one = r.stone_tree("main", 1);
        changes(&r, &one, 2)
    });
    assert_eq!(old_diffs.0, "A\ta.txt\n");

    // Everything that changes things is refused.
    let before = r.fingerprint();
    for args in [
        &["three"][..],
        &["1", "commit", "-m", "x"],
        &["commit-all"],
        &["2", "drop"],
    ] {
        let o = r.stone(args);
        assert_eq!(o.code, 1, "{args:?}: {}", o.stderr);
        assert!(
            o.stderr.contains("moved since the stones were made"),
            "{}",
            o.stderr
        );
    }
    r.assert_untouched(&before);

    // The plan changes nothing at all.
    let state = state_ref(&r);
    let plan = r.stone(&["repair"]);
    assert_eq!(plan.code, 0, "{}", plan.stderr);
    println!("{}", plan.stdout);
    assert!(plan.stdout.contains("moved since the stones were made"));
    assert!(plan.stdout.contains("new snapshots for Stones 1–2"));
    assert!(plan.stdout.contains("refs/stones/heads/main"));
    assert!(plan.stdout.contains("git stone repair --apply"));
    r.assert_untouched(&before);
    assert_eq!(state_ref(&r), state);

    // Apply: same stones, new trees, anchor at HEAD, every diff unchanged.
    let done = r.stone(&["repair", "--apply"]);
    assert_eq!(done.code, 0, "{}", done.stderr);
    println!("{}", done.stdout);
    assert!(done
        .stdout
        .starts_with("Repaired: the stones now sit on commit "));
    assert!(done.stdout.contains("No files were changed."));
    r.assert_untouched(&before);
    assert_eq!(anchor(&r), r.git(&["rev-parse", "HEAD"]).trim());
    let new_base = head_tree(&r);
    assert_eq!(changes(&r, &new_base, 1), old_diffs.0);
    let one = r.stone_tree("main", 1);
    assert_eq!(changes(&r, &one, 2), old_diffs.1);
    assert_ne!(one, r.git(&["rev-parse", "HEAD~1^{tree}"]).trim());
    // Names, ids and the reflog: the old state is still there.
    let list = r.ok(&["list"]);
    assert!(list.contains("one") && list.contains("two"), "{list}");
    let log = r.git(&["reflog", "show", "--format=%gs", "refs/stones/heads/main"]);
    assert_eq!(
        log,
        "git-stone: repair\ngit-stone: create 2\ngit-stone: create 1\n"
    );
    assert!(!r.ok(&["status"]).contains("moved"));

    // The chain works again: commit-all makes three commits' worth of history, files untouched.
    let files = r.files();
    r.ok(&["commit-all"]);
    assert_eq!(r.files(), files);
    let log = r.git(&["log", "--format=%s"]);
    assert_eq!(log, "two\none\nunrelated\nbase\n");
    assert!(r.git(&["status", "--porcelain"]).is_empty());
}

#[test]
fn repair_when_nothing_moved_or_nothing_exists() {
    let r = Repo::new();
    assert!(r.ok(&["repair"]).contains("no stones here"));
    r.write("a.txt", "a\n");
    r.ok(&["one"]);
    let before = r.fingerprint();
    assert!(r.ok(&["repair"]).contains("Nothing to repair"));
    assert!(r.ok(&["repair", "--apply"]).contains("Nothing to repair"));
    r.assert_untouched(&before);
}

#[test]
fn stones_that_match_commits_are_marked_by_repair_apply_not_by_the_plan() {
    let r = Repo::new();
    r.write("a.txt", "a\n");
    r.ok(&["one"]);
    r.git(&["add", "a.txt"]);
    r.git(&["commit", "-q", "-m", "by hand"]);
    let state = state_ref(&r);
    let plan = r.stone(&["repair"]);
    assert_eq!(plan.code, 0);
    assert!(
        plan.stdout.contains("Stone 1 already on the branch"),
        "{}",
        plan.stdout
    );
    assert_eq!(state_ref(&r), state);

    let done = r.stone(&["repair", "--apply"]);
    assert_eq!(done.code, 0, "{}", done.stderr);
    assert!(done.stderr.contains("Note: Stone 1 matches a commit"));
    assert!(done.stdout.contains("Nothing else to repair."));
    assert_ne!(state_ref(&r), state);
    assert!(r.ok(&["list"]).contains("No stones"), "{}", r.ok(&["list"]));
}

#[test]
fn a_partial_hand_commit_shrinks_the_stone_to_the_rest() {
    let r = Repo::new();
    r.write("a.txt", "a\n");
    r.write("b.txt", "b\n");
    r.ok(&["both"]);
    r.git(&["add", "a.txt"]);
    r.git(&["commit", "-q", "-m", "only a"]);
    assert_eq!(r.stone(&["commit-all"]).code, 1);

    let files = r.files();
    r.ok(&["repair", "--apply"]);
    assert_eq!(r.files(), files);
    assert_eq!(changes(&r, &head_tree(&r), 1), "A\tb.txt\n");
    assert_eq!(
        r.ok(&["diff", "1"])
            .lines()
            .filter(|l| l.starts_with("+b"))
            .count(),
        1
    );
    let before = r.git(&["rev-parse", "HEAD"]);
    r.ok(&["commit-all"]);
    assert_eq!(r.git(&["rev-parse", "HEAD~1"]), before);
    assert_eq!(
        r.git(&["diff", "--name-status", "HEAD~1", "HEAD"]),
        "A\tb.txt\n"
    );
}

#[test]
fn rebasing_onto_new_upstream_then_repair_does_not_revert_upstream() {
    let r = Repo::new();
    r.git(&["checkout", "-q", "-b", "feature"]);
    r.write("feat.txt", "feat\n");
    r.git(&["add", "feat.txt"]);
    r.git(&["commit", "-q", "-m", "feature work"]);
    // Uncommitted work: one edit to a tracked file, one new file.
    r.write("base.txt", "base\nmine\n");
    r.write("work.txt", "work\n");
    r.ok(&["wip"]);
    r.git(&["checkout", "-q", "main"]);
    r.write("up.txt", "upstream\n");
    r.git(&["add", "up.txt"]);
    r.git(&["commit", "-q", "-m", "upstream change"]);
    r.git(&["checkout", "-q", "feature"]);
    // checkout kept the edits; rebase with autostash replays the feature commit on top of upstream.
    r.git(&["rebase", "-q", "--autostash", "main"]);
    assert_eq!(r.read("base.txt"), "base\nmine\n");

    let stone_branch = "feature";
    let moved = r.stone(&["wip2"]);
    assert_eq!(moved.code, 1);
    assert!(moved.stderr.contains("moved since the stones were made"));

    let files = r.files();
    let done = r.stone(&["repair", "--apply"]);
    assert_eq!(done.code, 0, "{}{}", done.stdout, done.stderr);
    assert_eq!(r.files(), files);

    // The repaired stone sits on the rebased HEAD and carries the upstream file.
    let tree = r.git(&[
        "rev-parse",
        &format!("refs/stones/heads/{stone_branch}:trees/1"),
    ]);
    let listing = r.git(&["ls-tree", "-r", "--name-only", tree.trim()]);
    assert!(listing.lines().any(|l| l == "up.txt"), "{listing}");
    assert_eq!(
        r.git(&[
            "diff-tree",
            "-r",
            "--name-status",
            "HEAD^{tree}",
            tree.trim()
        ]),
        "M\tbase.txt\nA\twork.txt\n"
    );

    // Committing the stone adds only its own work; upstream's change stays.
    r.ok(&["commit-all"]);
    assert_eq!(
        r.git(&["diff-tree", "--no-commit-id", "-r", "--name-status", "HEAD"]),
        "M\tbase.txt\nA\twork.txt\n"
    );
    assert!(r
        .git(&["ls-tree", "-r", "--name-only", "HEAD"])
        .contains("up.txt"));
    assert_eq!(r.files(), files);
    assert!(r.git(&["status", "--porcelain"]).is_empty());
}

#[test]
fn reset_hard_then_repair_keeps_the_stone_work_on_the_older_base() {
    let r = Repo::new();
    r.write("second.txt", "second\n");
    r.git(&["add", "second.txt"]);
    r.git(&["commit", "-q", "-m", "second"]);
    r.write("edit.txt", "edit\n");
    r.ok(&["edit"]);
    let stone = r.stone_tree("main", 1);
    r.git(&["reset", "-q", "--hard", "HEAD~1"]);
    // Bring the lost work back from the stone first, as the plan says.
    r.git(&["restore", "--source", &stone, "--worktree", "."]);
    assert_eq!(r.read("second.txt"), "second\n");

    assert_eq!(r.stone(&["other"]).code, 1);
    let files = r.files();
    let done = r.stone(&["repair", "--apply"]);
    assert_eq!(done.code, 0, "{}{}", done.stdout, done.stderr);
    assert_eq!(r.files(), files);
    assert_eq!(anchor(&r), r.git(&["rev-parse", "HEAD"]).trim());
    // The stone keeps its own work on top of the older commit.
    assert_eq!(changes(&r, &head_tree(&r), 1), "A\tedit.txt\n");
}

#[test]
fn an_anchor_of_none_uses_the_empty_tree_as_the_old_base() {
    let r = Repo::unborn();
    r.write("a.txt", "a\n");
    r.ok(&["first"]);
    r.write("b.txt", "b\n");
    r.git(&["add", "b.txt"]);
    r.git(&["commit", "-q", "-m", "only b"]);
    assert_eq!(r.stone(&["second"]).code, 1);

    let done = r.stone(&["repair", "--apply"]);
    assert_eq!(done.code, 0, "{}{}", done.stdout, done.stderr);
    assert_eq!(changes(&r, &head_tree(&r), 1), "A\ta.txt\n");
    assert_eq!(anchor(&r), r.git(&["rev-parse", "HEAD"]).trim());
}

#[test]
fn a_conflicting_replay_changes_nothing_and_names_the_stone_and_paths() {
    let r = Repo::new();
    r.write("a.txt", "a\n");
    r.ok(&["fine"]);
    r.write("base.txt", "mine\n");
    r.ok(&["clashes"]);
    // The branch gets its own, different edit of base.txt.
    r.write("base.txt", "theirs\n");
    r.git(&["commit", "-q", "-am", "theirs"]);
    r.write("base.txt", "mine\n");

    let before = r.fingerprint();
    let state = state_ref(&r);
    for args in [&["repair"][..], &["repair", "--apply"]] {
        let o = r.stone(args);
        assert_eq!(o.code, 1, "{args:?}");
        println!("{}", o.stderr);
        assert!(
            o.stderr
                .starts_with("Stone 2 (clashes) cannot be put on top"),
            "{}",
            o.stderr
        );
        assert!(o.stderr.contains("\n  base.txt"));
        assert!(!o.stderr.contains("a.txt"));
        assert!(o.stderr.contains("Nothing was changed."));
        assert!(o.stdout.is_empty());
        r.assert_untouched(&before);
        assert_eq!(state_ref(&r), state);
        assert!(r.temp_indexes().is_empty());
    }
}

#[test]
fn a_branch_deleted_and_recreated_elsewhere_counts_as_moved() {
    let r = Repo::new();
    r.write("second.txt", "second\n");
    r.git(&["add", "second.txt"]);
    r.git(&["commit", "-q", "-m", "second"]);
    r.write("edit.txt", "edit\n");
    r.ok(&["edit"]);
    r.git(&["checkout", "-q", "-b", "tmp"]);
    r.git(&["branch", "-D", "main"]);
    r.git(&["branch", "main", "HEAD~1"]);
    r.git(&["checkout", "-q", "main"]);

    let status = r.ok(&["status"]);
    assert!(
        status.contains("moved since the stones were made"),
        "{status}"
    );
    let o = r.stone(&["again"]);
    assert_eq!(o.code, 1);
    assert!(o.stderr.contains("moved since the stones were made"));
    let plan = r.stone(&["repair"]);
    assert_eq!(plan.code, 0, "{}", plan.stderr);
    assert!(plan.stdout.contains("moved since the stones were made"));
}

#[test]
fn a_crash_before_the_write_leaves_the_state_alone() {
    let r = moved_by_an_unrelated_commit();
    let state = state_ref(&r);
    let before = r.fingerprint();
    let o = r.stone_env(
        &r.dir,
        &["repair", "--apply"],
        &[("GIT_STONE_FAILPOINT", "repair:before-cas")],
    );
    assert_eq!(o.code, 99);
    assert_eq!(state_ref(&r), state);
    r.assert_untouched(&before);
    assert!(r.temp_indexes().is_empty());
    // Running it again just works.
    assert_eq!(r.stone(&["repair", "--apply"]).code, 0);
}

#[test]
fn repair_apply_is_refused_during_a_merge_but_the_plan_still_works() {
    let r = moved_by_an_unrelated_commit();
    let head = r.git(&["rev-parse", "HEAD"]);
    std::fs::write(r.dir.join(".git/MERGE_HEAD"), head).unwrap();
    let state = state_ref(&r);
    let o = r.stone(&["repair", "--apply"]);
    assert_eq!(o.code, 1);
    assert!(o.stderr.contains("A merge is in progress"), "{}", o.stderr);
    assert_eq!(state_ref(&r), state);
    assert_eq!(r.stone(&["repair"]).code, 0);
}

#[test]
fn a_branch_without_commits_cannot_be_repaired() {
    let r = Repo::new();
    r.write("a.txt", "a\n");
    r.ok(&["one"]);
    r.git(&["update-ref", "-d", "refs/heads/main"]);
    let state = state_ref(&r);
    let o = r.stone(&["repair", "--apply"]);
    assert_eq!(o.code, 1);
    assert!(
        o.stderr.contains("The branch has no commits now"),
        "{}",
        o.stderr
    );
    assert_eq!(state_ref(&r), state);
}
