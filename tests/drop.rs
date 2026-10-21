mod common;

use common::Repo;

/// A repo with three stones, each adding one file, and a file of unstoned work.
fn three() -> Repo {
    let r = Repo::new();
    for (file, name) in [
        ("one.txt", "one"),
        ("two.txt", "two"),
        ("three.txt", "three"),
    ] {
        r.write(file, &format!("{file}\n"));
        r.ok(&[name, "-d", &format!("about {name}")]);
    }
    r.write("unstoned.txt", "u\n");
    r
}

fn trees(r: &Repo, ids: &[u64]) -> Vec<String> {
    ids.iter().map(|id| r.stone_tree("main", *id)).collect()
}

fn anchor_tree(r: &Repo) -> String {
    r.git(&["rev-parse", "HEAD^{tree}"]).trim().to_string()
}

fn list(r: &Repo) -> String {
    r.ok(&["list"])
}

fn list_text(r: &Repo) -> String {
    r.stone(&["list"]).stdout
}

/// I3: the diff of every surviving stone starts at the previous survivor (or the anchor).
fn assert_diffs_rebased(r: &Repo, ids: &[u64]) {
    let mut base = anchor_tree(r);
    for id in ids {
        let tree = r.stone_tree("main", *id);
        let shown = r.stone(&["diff", &id.to_string()]);
        assert_eq!(
            shown.stdout,
            r.git(&["diff", &base, &tree]),
            "diff of Stone {id}"
        );
        base = tree;
    }
}

#[test]
fn drop_the_middle_stone() {
    let r = three();
    let [t1, t3] = [r.stone_tree("main", 1), r.stone_tree("main", 3)];
    let before = r.fingerprint();
    let o = r.ok(&["drop", "2"]);
    assert_eq!(
        o,
        "Dropped Stone 2: two\n\nNo files were changed.\nStone 3 now includes all changes from the old Stone 2 boundary.\n"
    );
    r.assert_untouched(&before);
    assert_eq!(list(&r), "  1  one\n  3  three\n");
    assert_eq!(
        trees(&r, &[1, 3]),
        [t1.clone(), t3.clone()],
        "I2: trees unchanged"
    );
    assert_diffs_rebased(&r, &[1, 3]);
    // Stone 3 now covers two.txt as well.
    assert!(r.stone(&["diff", "3"]).stdout.contains("two.txt"));
    // The dropped stone's snapshot and description are gone from the state.
    assert!(!r.state_ref_exists("refs/stones/heads/main:trees/2"));
    assert!(!r.state_ref_exists("refs/stones/heads/main:desc/2"));
    assert!(r.state_ref_exists("refs/stones/heads/main:desc/3"));
}

#[test]
fn drop_the_first_stone() {
    let r = three();
    let before = r.fingerprint();
    let o = r.ok(&["2", "drop"]); // the other spelling, and not the first, so do first next
    assert!(o.starts_with("Dropped Stone 2"));
    r.assert_untouched(&before);

    let r = three();
    let t2 = r.stone_tree("main", 2);
    let before = r.fingerprint();
    let o = r.ok(&["drop", "1"]);
    assert!(
        o.contains("Stone 2 now includes all changes from the old Stone 1 boundary."),
        "{o}"
    );
    r.assert_untouched(&before);
    assert_eq!(r.stone_tree("main", 2), t2);
    // Stone 2 is now first, so it starts at the branch commit.
    assert_diffs_rebased(&r, &[2, 3]);
    assert!(r.ok(&["show", "2"]).contains("Base: branch commit "));
}

#[test]
fn drop_the_last_stone() {
    let r = three();
    let before = r.fingerprint();
    let o = r.ok(&["drop", "3"]);
    assert_eq!(
        o,
        "Dropped Stone 3: three\n\nNo files were changed.\nYour unstoned changes now include all changes from the old Stone 3 boundary.\n"
    );
    r.assert_untouched(&before);
    assert_diffs_rebased(&r, &[1, 2]);
    // The work is now unstoned.
    let status = r.ok(&["status"]);
    assert!(
        status.contains("Current changes since Stone 2:") && status.contains("A three.txt"),
        "{status}"
    );
}

#[test]
fn drop_everything_then_make_a_new_stone() {
    let r = three();
    let before = r.fingerprint();
    for id in ["1", "3", "2"] {
        r.ok(&["drop", id]);
        r.assert_untouched(&before);
    }
    assert_eq!(list(&r), "No stones yet.\n");
    assert_eq!(
        r.ok(&["list", "--porcelain"]),
        "git-stone-porcelain\t1\tlist\0"
    );
    // The empty chain keeps its ref, and with it the id counter (I7).
    assert!(r.state_ref_exists("refs/stones/heads/main"));
    assert!(r
        .git(&["cat-file", "-p", "refs/stones/heads/main:meta"])
        .contains("next-id 4\n"));
    let created = r.ok(&["fresh start"]);
    assert!(
        created.starts_with("Stone 4 created: fresh start"),
        "{created}"
    );
    assert_eq!(list(&r), "  4  fresh start\n");
    // All the work is in the new stone's diff, against the branch commit.
    assert_diffs_rebased(&r, &[4]);
}

#[test]
fn ids_are_never_reused_even_after_dropping_the_newest() {
    let r = three();
    r.ok(&["drop", "3"]);
    r.write("again.txt", "a\n");
    assert!(r.ok(&["again"]).starts_with("Stone 4 created"));
    assert_eq!(list(&r), "  1  one\n  2  two\n  4  again\n");
}

#[test]
fn bad_ids_change_nothing() {
    let r = three();
    let before = r.fingerprint();
    let state = r.git(&["rev-parse", "refs/stones/heads/main"]);
    for args in [
        vec!["drop", "9"],
        vec!["9", "drop"],
        vec!["drop", "x"],
        vec!["drop"],
    ] {
        let o = r.stone(&args);
        assert_eq!(o.code, 1, "{args:?}");
    }
    r.assert_untouched(&before);
    assert_eq!(r.git(&["rev-parse", "refs/stones/heads/main"]), state);
    let none = Repo::new();
    assert_eq!(none.stone(&["drop", "1"]).code, 1);
}

#[test]
fn the_old_state_stays_in_the_reflog() {
    let r = three();
    r.ok(&["drop", "2"]);
    let log = r.git(&["reflog", "show", "--format=%gs", "refs/stones/heads/main"]);
    assert_eq!(
        log,
        "git-stone: drop 2\ngit-stone: create 3\ngit-stone: create 2\ngit-stone: create 1\n"
    );
    // Stone 2's tree can still be recovered from the previous state.
    let old = r.git(&["rev-parse", "refs/stones/heads/main@{1}:trees/2"]);
    assert_eq!(old.trim().len(), 40);
}

#[test]
fn drop_works_during_a_merge_but_create_does_not() {
    let r = Repo::new();
    r.write("f.txt", "base\n");
    r.git(&["add", "f.txt"]);
    r.git(&["commit", "-q", "-m", "f"]);
    r.git(&["checkout", "-q", "-b", "topic"]);
    r.write("f.txt", "topic\n");
    r.git(&["commit", "-q", "-am", "topic"]);
    r.git(&["checkout", "-q", "main"]);
    r.write("f.txt", "main\n");
    r.git(&["commit", "-q", "-am", "main"]);
    // Stones are made at the tip of main, then a conflicting merge starts without moving it.
    r.write("a.txt", "a\n");
    r.ok(&["a"]);
    r.write("b.txt", "b\n");
    r.ok(&["b"]);
    assert_ne!(r.git_try(&["merge", "topic"]).code, 0);
    assert!(r.stone(&["c"]).stderr.contains("A merge is in progress"));

    let o = r.stone(&["drop", "1"]);
    assert_eq!(o.code, 0, "{}", o.stderr);
    assert_eq!(list(&r), "  2  b\n");
}

#[test]
fn drop_works_when_the_branch_moved() {
    let r = three();
    r.git(&["commit", "-q", "--allow-empty", "-m", "someone else"]);
    let before = r.fingerprint();
    let o = r.stone(&["drop", "2"]);
    assert_eq!(o.code, 0, "{}", o.stderr);
    assert!(o.stderr.contains("the branch moved"), "{}", o.stderr);
    assert!(o.stdout.contains("No files were changed"), "{}", o.stdout);
    // Only the record is gone: no file, no commit, no index change.
    r.assert_untouched(&before);
    assert_eq!(list_text(&r), "  1  one\n  3  three\n");
}

#[test]
fn drop_all_removes_every_boundary_and_keeps_the_ids_moving() {
    let r = three();
    let before = r.fingerprint();
    let o = r.stone(&["drop", "all"]);
    assert_eq!(o.code, 0, "{}", o.stderr);
    assert!(o.stdout.contains("Dropped 3 stones."), "{}", o.stdout);
    r.assert_untouched(&before);
    assert_eq!(list_text(&r), "No stones yet.\n");
    // The ids are never reused (I7).
    assert!(r.ok(&["next"]).starts_with("Stone 4 created"));
}

#[test]
fn drop_all_works_when_the_branch_moved_and_refuses_when_there_is_nothing() {
    let r = three();
    r.git(&["commit", "-q", "--allow-empty", "-m", "someone else"]);
    let o = r.stone(&["drop", "all"]);
    assert_eq!(o.code, 0, "{}", o.stderr);
    assert_eq!(list_text(&r), "No stones yet.\n");
    let again = r.stone(&["drop", "all"]);
    assert_eq!(again.code, 1);
    assert!(again.stderr.contains("nothing to drop"), "{}", again.stderr);
}

#[test]
fn drop_failpoint_leaves_the_state_alone() {
    let r = three();
    let state = r.git(&["rev-parse", "refs/stones/heads/main"]);
    let before = r.fingerprint();
    let o = r.stone_env(
        &r.dir,
        &["drop", "2"],
        &[("GIT_STONE_FAILPOINT", "drop:before-cas")],
    );
    assert_eq!(o.code, 99, "{}{}", o.stdout, o.stderr);
    assert_eq!(r.git(&["rev-parse", "refs/stones/heads/main"]), state);
    r.assert_untouched(&before);
    assert_eq!(list(&r), "  1  one\n  2  two\n  3  three\n");
    r.ok(&["drop", "2"]);
}

#[test]
fn drop_on_a_detached_head_and_an_unborn_branch() {
    let r = Repo::new();
    r.git(&["checkout", "-q", "--detach"]);
    r.write("d.txt", "d\n");
    r.ok(&["d1"]);
    r.write("e.txt", "e\n");
    r.ok(&["d2"]);
    r.ok(&["drop", "1"]);
    assert_eq!(list(&r), "  2  d2\n");

    let u = Repo::unborn();
    u.write("a.txt", "a\n");
    u.ok(&["a"]);
    u.write("b.txt", "b\n");
    u.ok(&["b"]);
    u.ok(&["drop", "1"]);
    assert_eq!(list(&u), "  2  b\n");
    // With the first stone gone, Stone 2 is based on the empty tree again.
    let empty = u.git(&["hash-object", "-t", "tree", "--stdin"]);
    let tree = u.stone_tree("main", 2);
    assert_eq!(
        u.stone(&["diff", "2"]).stdout,
        u.git(&["diff", empty.trim(), &tree])
    );
}

#[test]
fn drop_of_two_at_once_loses_cleanly() {
    let r = three();
    let results: Vec<_> = std::thread::scope(|s| {
        let handles: Vec<_> = ["1", "3"]
            .iter()
            .map(|id| {
                let r = &r;
                s.spawn(move || r.stone(&["drop", id]))
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });
    for o in &results {
        assert!(
            o.code == 0
                || (o.code == 1
                    && o.stderr
                        .contains("Another git stone command changed the stones at the same time")),
            "{}{}",
            o.stdout,
            o.stderr
        );
    }
    // Whatever happened, the chain is readable and consistent.
    let remaining = list(&r);
    let kept = remaining.lines().count();
    assert_eq!(
        kept,
        3 - results.iter().filter(|o| o.code == 0).count(),
        "{remaining}"
    );
    r.git(&["fsck", "--no-dangling"]);
}
