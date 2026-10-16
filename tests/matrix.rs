//! One test per row of the Git interaction table (A11) and the loss guards (A14).
//! Every test ends with `healthy`: `git fsck --no-dangling` is clean and the stones still load.

mod common;

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::time::{Duration, SystemTime};

use common::Repo;

fn healthy(r: &Repo) {
    r.git(&["fsck", "--no-dangling"]);
    let listed = r.stone(&["list"]);
    assert_eq!(listed.code, 0, "{}", listed.stderr);
}

fn state(r: &Repo) -> String {
    r.git(&["rev-parse", "refs/stones/heads/main"])
        .trim()
        .to_string()
}

fn one_stone() -> Repo {
    let r = Repo::new();
    r.write("a.txt", "a\n");
    r.ok(&["work", "-d", "details"]);
    r
}

/// `main` has `base` and `second`; `other` branches from `base` and adds `o.txt`. One stone on `main`.
fn two_lines() -> Repo {
    let r = Repo::new();
    r.write("second.txt", "second\n");
    r.git(&["add", "second.txt"]);
    r.git(&["commit", "-q", "-m", "second"]);
    r.git(&["branch", "other", "HEAD~1"]);
    r.git(&["checkout", "-q", "other"]);
    r.write("o.txt", "o\n");
    r.git(&["add", "o.txt"]);
    r.git(&["commit", "-q", "-m", "other work"]);
    r.git(&["checkout", "-q", "main"]);
    r.write("a.txt", "a\n");
    r.ok(&["work"]);
    r
}

/// The branch moved under the stones: warn, refuse to change, still read, and repair puts it right.
fn assert_moved_then_repaired(r: &Repo) {
    let status = r.ok(&["status"]);
    assert!(
        status.contains("moved since the stones were made"),
        "{status}"
    );
    let before = state(r);
    let refused = r.stone(&["again"]);
    assert_eq!(refused.code, 1);
    assert!(refused.stderr.contains("moved since the stones were made"));
    assert_eq!(r.stone(&["1", "commit", "-m", "x"]).code, 1);
    assert_eq!(state(r), before);
    healthy(r);
    let done = r.stone(&["repair", "--apply"]);
    assert_eq!(done.code, 0, "{}{}", done.stdout, done.stderr);
    assert!(!r.ok(&["status"]).contains("moved since"));
    healthy(r);
}

// ---- add, restore, stash ----

#[test]
fn add_and_restore_never_touch_the_stones() {
    let r = one_stone();
    let before = state(&r);
    r.write("base.txt", "base\nmore\n");
    r.write("b.txt", "b\n");
    assert!(r.ok(&["status"]).contains("b.txt"));
    r.git(&["add", "base.txt", "b.txt"]);
    assert_eq!(state(&r), before);
    r.git(&["restore", "--staged", "base.txt"]);
    r.git(&["restore", "base.txt"]);
    assert_eq!(r.read("base.txt"), "base\n");
    assert_eq!(state(&r), before);
    // Status recomputes: the restored file is no longer a change.
    let status = r.ok(&["status"]);
    assert!(!status.contains("base.txt"), "{status}");
    healthy(&r);
}

#[test]
fn stash_never_touches_stones_and_creating_after_it_is_refused() {
    let r = one_stone();
    let before = state(&r);
    r.git(&["stash", "-u", "-q"]);
    assert!(r.state_ref_exists("refs/stash"));
    assert_eq!(state(&r), before);
    assert!(r.ok(&["status"]).contains("Did you stash?"));
    let refused = r.stone(&["after the stash"]);
    assert_eq!(refused.code, 1);
    assert_eq!(state(&r), before);
    r.git(&["stash", "pop", "-q"]);
    assert!(!r.ok(&["status"]).contains("Did you stash?"));
    healthy(&r);
}

// ---- the branch moves ----

#[test]
fn an_ordinary_commit_blocks_unless_it_is_exactly_the_stone() {
    // Not the stone: blocked.
    let r = one_stone();
    r.write("x.txt", "x\n");
    r.git(&["add", "x.txt"]);
    r.git(&["commit", "-q", "-m", "x"]);
    assert_moved_then_repaired(&r);

    // Exactly the stone: marked committed, nothing blocks.
    let r = one_stone();
    r.git(&["add", "a.txt"]);
    r.git(&["commit", "-q", "-m", "by hand"]);
    assert!(r.ok(&["status"]).contains("already on the branch"));
    let o = r.stone(&["next"]);
    assert_eq!(o.code, 1, "an empty working tree has nothing to stone");
    assert!(o.stderr.contains("Note: Stone 1 matches a commit"));
    healthy(&r);
}

#[test]
fn amend_reset_rebase_pull_merge_cherry_pick_and_am_all_move_the_branch() {
    type Action = (&'static str, fn(&Repo));
    let actions: [Action; 7] = [
        ("amend", |r| {
            r.git(&["commit", "-q", "--amend", "--allow-empty", "-m", "reworded"]);
        }),
        ("reset", |r| {
            r.git(&["reset", "-q", "--hard", "HEAD~1"]);
        }),
        ("rebase", |r| {
            r.git(&["rebase", "-q", "other"]);
        }),
        ("pull", |r| {
            r.git(&["pull", "-q", "--no-rebase", "--no-edit", ".", "other"]);
        }),
        ("merge", |r| {
            r.git(&["merge", "-q", "--no-ff", "-m", "merged", "other"]);
        }),
        ("cherry-pick", |r| {
            r.git(&["cherry-pick", "other"]);
        }),
        ("am", |r| {
            let patch = r.git(&["format-patch", "-1", "other", "--stdout"]);
            let file = r.home().join("p.patch");
            fs::write(&file, patch).unwrap();
            r.git(&["am", "-q", file.to_str().unwrap()]);
        }),
    ];
    for (name, act) in actions {
        let r = two_lines();
        act(&r);
        println!("--- {name}");
        assert_moved_then_repaired(&r);
    }
}

#[test]
fn a_branch_deleted_and_recreated_elsewhere_is_a_move() {
    let r = two_lines();
    r.git(&["checkout", "-q", "other"]);
    r.git(&["branch", "-D", "main"]);
    r.git(&["branch", "main", "other"]);
    r.git(&["checkout", "-q", "main"]);
    assert_moved_then_repaired(&r);
}

// ---- operations in progress ----

#[test]
fn a_conflicted_cherry_pick_refuses_changes_but_reads_and_drops_work() {
    let r = Repo::new();
    r.git(&["branch", "other"]);
    r.write("base.txt", "main\n");
    r.git(&["commit", "-q", "-am", "main edit"]);
    r.git(&["checkout", "-q", "other"]);
    r.write("base.txt", "other\n");
    r.git(&["commit", "-q", "-am", "other edit"]);
    r.git(&["checkout", "-q", "main"]);
    r.write("a.txt", "a\n");
    r.ok(&["one"]);
    r.write("b.txt", "b\n");
    r.ok(&["two"]);
    assert_ne!(r.git_try(&["cherry-pick", "other"]).code, 0);

    for args in [&["three"][..], &["1", "commit", "-m", "x"], &["commit-all"]] {
        let o = r.stone(args);
        assert_eq!(o.code, 1, "{args:?}: {}", o.stderr);
        assert!(
            o.stderr.contains("in progress") || o.stderr.contains("merge conflicts"),
            "{}",
            o.stderr
        );
    }
    let status = r.ok(&["status"]);
    assert!(status.contains("cherry-pick is in progress"), "{status}");
    assert_eq!(r.stone(&["list"]).code, 0);
    assert_eq!(r.stone(&["show", "1"]).code, 0);
    assert_eq!(r.stone(&["1", "drop"]).code, 0);
    r.git(&["cherry-pick", "--abort"]);
    assert!(r.ok(&["list"]).contains("two"));
    healthy(&r);
}

#[test]
fn a_stopped_rebase_refuses_changes_and_the_stones_are_back_after_abort() {
    let r = Repo::new();
    r.git(&["branch", "other"]);
    r.write("base.txt", "main\n");
    r.git(&["commit", "-q", "-am", "main edit"]);
    r.git(&["checkout", "-q", "other"]);
    r.write("base.txt", "other\n");
    r.git(&["commit", "-q", "-am", "other edit"]);
    r.git(&["checkout", "-q", "main"]);
    r.write("a.txt", "a\n");
    r.ok(&["one"]);
    let before = state(&r);
    assert_ne!(r.git_try(&["rebase", "other"]).code, 0);

    let o = r.stone(&["two"]);
    assert_eq!(o.code, 1);
    assert!(o.stderr.contains("rebase"), "{}", o.stderr);
    assert_eq!(r.stone(&["commit-all"]).code, 1);
    assert_eq!(r.stone(&["status"]).code, 0);
    assert_eq!(r.stone(&["list"]).code, 0);
    r.git(&["rebase", "--abort"]);
    assert_eq!(state(&r), before);
    assert!(r.ok(&["list"]).contains("one"));
    healthy(&r);
}

// ---- losing work, switching, renaming ----

#[test]
fn work_lost_to_reset_and_clean_comes_back_from_the_stone() {
    let r = Repo::new();
    r.write("base.txt", "base\nedited\n");
    r.write("dir/new.txt", "new\n");
    r.ok(&["precious"]);
    r.git(&["reset", "-q", "--hard"]);
    r.git(&["clean", "-fdq"]);
    assert_eq!(r.read("base.txt"), "base\n");
    assert!(!r.dir.join("dir/new.txt").exists());
    // Stones keep it, and the documented way brings it back.
    r.git(&["checkout", "refs/stones/heads/main:trees/1", "--", "."]);
    assert_eq!(r.read("base.txt"), "base\nedited\n");
    assert_eq!(r.read("dir/new.txt"), "new\n");
    assert!(r.ok(&["list"]).contains("precious"));
    healthy(&r);
}

#[test]
fn switching_away_and_back_loses_nothing_and_never_shows_the_other_chain() {
    let r = one_stone();
    let before = state(&r);
    r.git(&["stash", "-u", "-q"]);
    r.git(&["checkout", "-q", "-b", "side"]);
    assert!(r.ok(&["list"]).contains("No stones"), "{}", r.ok(&["list"]));
    r.write("s.txt", "s\n");
    r.ok(&["side work"]);
    assert!(!r.ok(&["list"]).contains("work\n") || r.ok(&["list"]).contains("side work"));
    r.git(&["checkout", "-q", "main"]);
    r.git(&["stash", "pop", "-q"]);
    assert_eq!(state(&r), before);
    let list = r.ok(&["list"]);
    assert!(
        list.contains("work") && !list.contains("side work"),
        "{list}"
    );
    healthy(&r);
}

#[test]
fn a_renamed_branch_leaves_an_orphan_warning_and_adopt_fixes_it() {
    let r = one_stone();
    r.git(&["branch", "-m", "main", "trunk"]);
    let status = r.ok(&["status"]);
    assert!(status.contains("adopt main"), "{status}");
    r.ok(&["adopt", "main"]);
    assert!(!r.ok(&["status"]).contains("adopt main"));
    assert!(r.ok(&["list"]).contains("work"));
    healthy(&r);
}

// ---- worktrees ----

#[test]
fn worktrees_never_see_each_others_stones() {
    let r = one_stone();
    let twin = r.tmp.path().join("twin");
    r.git(&[
        "worktree",
        "add",
        "-q",
        "-b",
        "side",
        twin.to_str().unwrap(),
    ]);
    // The linked worktree starts with no stones of its own and none of main's.
    let o = r.stone_in(&twin, &["list"]);
    assert_eq!(o.code, 0);
    assert!(o.stdout.contains("No stones"), "{}", o.stdout);
    fs::write(twin.join("t.txt"), "t\n").unwrap();
    assert_eq!(r.stone_in(&twin, &["twin work"]).code, 0);
    assert!(r.stone_in(&twin, &["list"]).stdout.contains("twin work"));
    let main = r.ok(&["list"]);
    assert!(
        main.contains("work") && !main.contains("twin work"),
        "{main}"
    );
    // Each worktree's real index and files were left alone by the other's commands.
    assert!(!r.dir.join("t.txt").exists() && !twin.join("a.txt").exists());

    // The same branch in two worktrees is a warning, not an error.
    let third = r.tmp.path().join("third");
    r.git(&[
        "worktree",
        "add",
        "-q",
        "--force",
        third.to_str().unwrap(),
        "main",
    ]);
    assert!(r
        .ok(&["status"])
        .contains("checked out in more than one worktree"));
    healthy(&r);
}

/// A linked worktree with a detached HEAD and one stone. Returns the stone's tree.
fn detached_twin(r: &Repo) -> (std::path::PathBuf, String) {
    let twin = r.tmp.path().join("twin");
    r.git(&["worktree", "add", "-q", "--detach", twin.to_str().unwrap()]);
    fs::write(twin.join("t.txt"), "t\n").unwrap();
    assert_eq!(r.stone_in(&twin, &["twin work", "-d", "kept"]).code, 0);
    // Its own ref, named after the worktree: the main worktree has none.
    assert!(r.state_ref_exists("refs/stones/detached/linked/twin"));
    assert!(!r.state_ref_exists("refs/stones/detached/main"));
    let tree = r
        .git_in(
            &twin,
            &["rev-parse", "refs/stones/detached/linked/twin:trees/1"],
        )
        .trim()
        .to_string();
    (twin, tree)
}

#[test]
fn a_detached_chain_survives_gc_in_its_own_worktree_and_with_reflogs() {
    let r = Repo::new();
    let (twin, tree) = detached_twin(&r);
    // The reflog of the ref protects it while it is young, even for gc run from another worktree.
    r.git(&["-c", "gc.reflogExpire=never", "gc", "-q", "--prune=now"]);
    r.git(&["cat-file", "-e", &tree]);
    // Reflogs gone: gc run in the worktree that owns the ref still sees the ref.
    r.git_in(&twin, &["reflog", "expire", "--expire=now", "--all"]);
    r.git_in(&twin, &["gc", "-q", "--prune=now"]);
    r.git(&["cat-file", "-e", &tree]);
    let shown = r.stone_in(&twin, &["show", "1"]);
    assert_eq!(shown.code, 0, "{}", shown.stderr);
    assert!(shown.stdout.contains("kept"));
    r.git_in(&twin, &["fsck", "--no-dangling"]);
    healthy(&r);
}

/// A4 VERIFY: `refs/worktree/*` is not a gc root for other worktrees, so detached chains live in a
/// shared ref (`refs/stones/detached/...`) instead. This is the case that lost data before.
#[test]
fn a_detached_chain_survives_gc_run_in_another_worktree_after_reflog_expiry() {
    let r = Repo::new();
    let (twin, tree) = detached_twin(&r);
    r.git(&["reflog", "expire", "--expire=now", "--all"]);
    r.git_in(&twin, &["reflog", "expire", "--expire=now", "--all"]);
    r.git(&["gc", "-q", "--prune=now"]);
    r.git(&["cat-file", "-e", &tree]);
    assert_eq!(r.stone_in(&twin, &["show", "1"]).code, 0);
    // The same for a chain on the main worktree's own detached HEAD, with gc run from a linked one.
    r.git(&["checkout", "-q", "--detach"]);
    r.write(
        "m.txt", "m
",
    );
    r.ok(&["main detached"]);
    let main_tree = r
        .git(&["rev-parse", "refs/stones/detached/main:trees/1"])
        .trim()
        .to_string();
    r.git(&["reflog", "expire", "--expire=now", "--all"]);
    r.git_in(&twin, &["reflog", "expire", "--expire=now", "--all"]);
    r.git_in(&twin, &["gc", "-q", "--prune=now"]);
    r.git(&["cat-file", "-e", &main_tree]);
    assert!(r.ok(&["list"]).contains("main detached"));
    healthy(&r);
}

// ---- gc ----

#[test]
fn gc_collects_dropped_stones_but_keeps_live_ones() {
    let r = Repo::new();
    r.write("a.txt", "only in stone one\n");
    r.ok(&["first", "-d", "the first description"]);
    r.write("b.txt", "b\n");
    r.ok(&["second"]);
    let dropped_tree = r.stone_tree("main", 1);
    let live_tree = r.stone_tree("main", 2);
    let dropped_desc = r
        .git(&["rev-parse", "refs/stones/heads/main:desc/1"])
        .trim()
        .to_string();
    r.ok(&["1", "drop"]);

    // While the reflog remembers the old state, the dropped stone is still reachable.
    // (The test clock is fixed in the past, so gc must be told not to expire by age.)
    r.git(&[
        "-c",
        "gc.reflogExpire=never",
        "-c",
        "gc.reflogExpireUnreachable=never",
        "gc",
        "-q",
        "--prune=now",
    ]);
    r.git(&["cat-file", "-e", &dropped_tree]);
    // Once it expires, only the live stone stays.
    r.git(&["reflog", "expire", "--expire=now", "--all"]);
    r.git(&["gc", "-q", "--prune=now"]);
    assert_ne!(r.git_try(&["cat-file", "-e", &dropped_tree]).code, 0);
    assert_ne!(r.git_try(&["cat-file", "-e", &dropped_desc]).code, 0);
    r.git(&["cat-file", "-e", &live_tree]);
    assert!(r.ok(&["list"]).contains("second"));
    assert!(r.ok(&["review"]).contains("second"));
    healthy(&r);
}

// ---- hooks ----

#[test]
fn no_verify_skips_hooks_and_a_failing_hook_changes_nothing() {
    let r = one_stone();
    r.hook("pre-commit", "echo no >&2; exit 1");
    let before = (state(&r), r.git(&["rev-parse", "HEAD"]));
    let o = r.stone(&["1", "commit", "-m", "x"]);
    assert_ne!(o.code, 0);
    assert_eq!((state(&r), r.git(&["rev-parse", "HEAD"])), before);
    healthy(&r);
    let o = r.stone(&["1", "commit", "-m", "x", "--no-verify"]);
    assert_eq!(o.code, 0, "{}{}", o.stdout, o.stderr);
    assert_ne!(r.git(&["rev-parse", "HEAD"]), before.1);
    healthy(&r);
}

// ---- failures ----

fn set_mode(path: &Path, mode: u32) {
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
}

/// Make every directory under `path` (and itself) read-only, or writable again.
fn lock_dirs(path: &Path, mode: u32) {
    if path.is_dir() {
        if mode == 0o755 {
            set_mode(path, mode);
        }
        for entry in fs::read_dir(path).unwrap() {
            lock_dirs(&entry.unwrap().path(), mode);
        }
        if mode != 0o755 {
            set_mode(path, mode);
        }
    }
}

#[test]
fn a_full_object_store_fails_cleanly_and_changes_nothing() {
    let r = Repo::new();
    r.write("a.txt", "a\n");
    r.ok(&["first"]);
    let before = state(&r);
    r.write("b.txt", "never stored\n");
    let files = r.files();
    let objects = r.dir.join(".git/objects");
    lock_dirs(&objects, 0o555);
    let o = r.stone(&["second"]);
    lock_dirs(&objects, 0o755);
    assert_eq!(o.code, 2, "{}{}", o.stdout, o.stderr);
    assert!(o.stderr.starts_with("git-stone: "), "{}", o.stderr);
    assert_eq!(state(&r), before);
    assert_eq!(r.files(), files);
    assert!(r.temp_indexes().is_empty());
    healthy(&r);
    // Space is back: the same command works.
    assert_eq!(r.stone(&["second"]).code, 0);
}

#[test]
fn a_ref_that_cannot_be_written_fails_cleanly_and_changes_nothing() {
    let r = one_stone();
    let before = state(&r);
    r.write("b.txt", "b\n");
    let heads = r.dir.join(".git/refs/stones/heads");
    set_mode(&heads, 0o555);
    let created = r.stone(&["second"]);
    let dropped = r.stone(&["1", "drop"]);
    set_mode(&heads, 0o755);
    for o in [created, dropped] {
        assert_eq!(o.code, 2, "{}{}", o.stdout, o.stderr);
        assert!(o.stderr.starts_with("git-stone: "), "{}", o.stderr);
    }
    assert_eq!(state(&r), before);
    assert!(r.temp_indexes().is_empty());
    healthy(&r);
    assert_eq!(r.stone(&["second"]).code, 0);
}

#[test]
fn a_damaged_state_ref_is_reported_not_guessed_at() {
    let r = one_stone();
    let files = r.files();
    // The ref points at an ordinary commit: no meta, no trees.
    r.git(&["update-ref", "refs/stones/heads/main", "HEAD"]);
    for args in [
        &["list"][..],
        &["status"],
        &["new"],
        &["1", "drop"],
        &["repair"],
    ] {
        let o = r.stone(args);
        assert_eq!(o.code, 2, "{args:?}: {}{}", o.stdout, o.stderr);
        assert!(o.stderr.contains("stone state is corrupt"), "{}", o.stderr);
    }
    assert_eq!(r.files(), files);
    r.git(&["fsck", "--no-dangling"]);
}

#[test]
fn old_temp_indexes_are_cleaned_up_and_young_ones_are_not() {
    let r = one_stone();
    let old = r.dir.join(".git/stone-index-1-2-3");
    let young = r.dir.join(".git/stone-index-4-5-6");
    for path in [&old, &young] {
        fs::write(path, "junk").unwrap();
    }
    let two_hours_ago = SystemTime::now() - Duration::from_secs(2 * 3600);
    fs::File::options()
        .write(true)
        .open(&old)
        .unwrap()
        .set_modified(two_hours_ago)
        .unwrap();
    r.write("b.txt", "b\n");
    r.ok(&["second"]);
    assert!(
        !old.exists(),
        "a leftover from a killed run should be removed"
    );
    assert!(
        young.exists(),
        "a recent one may belong to a running command"
    );
    healthy(&r);
}
