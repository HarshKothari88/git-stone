mod common;

use common::Repo;

fn oid(r: &Repo, name: &str) -> String {
    r.git(&["rev-parse", name]).trim().to_string()
}

fn exists(r: &Repo, name: &str) -> bool {
    r.state_ref_exists(name)
}

/// Two stones with details, made on a detached HEAD.
fn detached_with_two_stones() -> Repo {
    let r = Repo::new();
    r.git(&["checkout", "-q", "--detach"]);
    r.write("a.txt", "a\n");
    r.ok(&["first", "-d", "Why the first one exists."]);
    r.write("b.txt", "b\n");
    r.ok(&["second"]);
    r
}

#[test]
fn detached_stones_follow_a_new_branch() {
    let r = detached_with_two_stones();
    let trees = [
        oid(&r, "refs/stones/detached/main:trees/1"),
        oid(&r, "refs/stones/detached/main:trees/2"),
    ];
    r.git(&["switch", "-q", "-c", "feat"]);
    let status = r.ok(&["status"]);
    assert!(status.contains("git stone adopt detached"), "{status}");

    let before = r.fingerprint();
    let out = r.ok(&["adopt", "detached"]);
    assert_eq!(
        out,
        "Moved 2 stones from detached HEAD to branch `feat`.\nNo files were changed.\n"
    );
    r.assert_untouched(&before);

    // The chain is on the branch, the detached ref is gone, the trees are the same objects.
    assert!(exists(&r, "refs/stones/heads/feat"));
    assert!(!exists(&r, "refs/stones/detached/main"));
    assert_eq!(r.stone_tree("feat", 1), trees[0]);
    assert_eq!(r.stone_tree("feat", 2), trees[1]);
    let meta = r.git(&["cat-file", "-p", "refs/stones/heads/feat:meta"]);
    assert!(meta.contains("branch refs/heads/feat\n"), "{meta}");
    assert!(meta.contains("next-id 3\n"));
    let list = r.ok(&["list"]);
    assert!(list.contains("first") && list.contains("second"), "{list}");
    assert!(r.ok(&["show", "1"]).contains("Why the first one exists."));
    assert!(!r.ok(&["status"]).contains("adopt"));

    // The reflog says where it came from.
    let log = r.git(&["reflog", "show", "--format=%gs", "refs/stones/heads/feat"]);
    assert_eq!(log, "git-stone: adopt 2 from detached\n");

    // And the stones are usable at once.
    r.ok(&["commit-all"]);
    assert_eq!(r.git(&["log", "--format=%s", "-2"]), "second\nfirst\n");
}

#[test]
fn a_renamed_branch_gets_its_stones_back() {
    let r = Repo::new();
    r.write("a.txt", "a\n");
    r.ok(&["work", "-d", "details"]);
    let tree = r.stone_tree("main", 1);
    r.git(&["branch", "-m", "main", "trunk"]);
    let status = r.ok(&["status"]);
    assert!(status.contains("git stone adopt main"), "{status}");

    let out = r.ok(&["adopt", "main"]);
    assert_eq!(
        out,
        "Moved 1 stone from branch `main` to branch `trunk`.\nNo files were changed.\n"
    );
    assert!(!exists(&r, "refs/stones/heads/main"));
    assert_eq!(r.stone_tree("trunk", 1), tree);
    assert!(r.ok(&["show", "1"]).contains("details"));
    assert!(!r.ok(&["status"]).contains("adopt"));
}

#[test]
fn ids_are_kept_when_they_cannot_collide() {
    let r = detached_with_two_stones();
    r.ok(&["1", "drop"]);
    r.git(&["switch", "-q", "-c", "feat"]);
    let out = r.ok(&["adopt", "detached"]);
    assert_eq!(
        out,
        "Moved 1 stone from detached HEAD to branch `feat`.\nNo files were changed.\n"
    );
    let list = r.ok(&["list"]);
    assert!(list.contains("second") && !list.contains("first"), "{list}");
    // Still Stone 2, and the next one is Stone 3.
    r.write("c.txt", "c\n");
    assert!(r.ok(&["third"]).starts_with("Stone 3 created"));
}

#[test]
fn ids_are_renumbered_when_the_target_used_them_before() {
    let r = Repo::new();
    r.git(&["checkout", "-q", "--detach"]);
    r.write("d1.txt", "1\n");
    r.ok(&["d-one"]);
    r.write("d2.txt", "2\n");
    r.ok(&["d-two"]);
    let trees = [
        oid(&r, "refs/stones/detached/main:trees/1"),
        oid(&r, "refs/stones/detached/main:trees/2"),
    ];
    r.git(&["switch", "-q", "-c", "feat"]);
    // The branch used ids 1 and 2 before and has none left now (next id is 3).
    r.write("x1.txt", "x\n");
    r.ok(&["x-one"]);
    r.write("x2.txt", "x\n");
    r.ok(&["x-two"]);
    r.ok(&["1", "drop"]);
    r.ok(&["2", "drop"]);
    let state = oid(&r, "refs/stones/heads/feat");

    let out = r.ok(&["adopt", "detached"]);
    assert_eq!(
        out,
        "Moved 2 stones from detached HEAD to branch `feat`.\nStone 1 is now Stone 3.\nStone 2 is now Stone 4.\nNo files were changed.\n"
    );
    assert_ne!(oid(&r, "refs/stones/heads/feat"), state);
    assert_eq!(r.stone_tree("feat", 3), trees[0]);
    assert_eq!(r.stone_tree("feat", 4), trees[1]);
    let meta = r.git(&["cat-file", "-p", "refs/stones/heads/feat:meta"]);
    assert!(meta.contains("next-id 5\n"), "{meta}");
    let list = r.ok(&["list"]);
    assert!(list.contains("d-one") && list.contains("d-two"), "{list}");
}

#[test]
fn it_is_refused_when_the_target_has_stones() {
    let r = detached_with_two_stones();
    r.git(&["switch", "-q", "-c", "feat"]);
    r.write("mine.txt", "m\n");
    r.ok(&["mine"]);
    let (source, target) = (
        oid(&r, "refs/stones/detached/main"),
        oid(&r, "refs/stones/heads/feat"),
    );
    let before = r.fingerprint();
    let o = r.stone(&["adopt", "detached"]);
    assert_eq!(o.code, 1);
    assert!(
        o.stderr
            .contains("There are already stones on branch `feat`"),
        "{}",
        o.stderr
    );
    assert_eq!(oid(&r, "refs/stones/detached/main"), source);
    assert_eq!(oid(&r, "refs/stones/heads/feat"), target);
    r.assert_untouched(&before);
}

#[test]
fn it_is_refused_when_there_is_nothing_to_move() {
    let r = Repo::new();
    // No chain at all.
    let o = r.stone(&["adopt", "nosuch"]);
    assert_eq!(o.code, 1);
    assert_eq!(
        o.stderr,
        "There are no stones to move from branch `nosuch`.\n"
    );
    let o = r.stone(&["adopt", "detached"]);
    assert_eq!(o.code, 1);
    assert!(o
        .stderr
        .contains("There are no stones to move from detached HEAD."));

    // A chain that is empty because every stone was dropped.
    r.git(&["checkout", "-q", "-b", "old"]);
    r.write("a.txt", "a\n");
    r.ok(&["gone"]);
    r.ok(&["1", "drop"]);
    r.git(&["checkout", "-q", "main"]);
    let before = oid(&r, "refs/stones/heads/old");
    let o = r.stone(&["adopt", "old"]);
    assert_eq!(o.code, 1);
    assert!(o
        .stderr
        .contains("There are no stones to move from branch `old`."));
    assert_eq!(oid(&r, "refs/stones/heads/old"), before);
}

#[test]
fn it_refuses_the_scope_you_are_already_on_and_bad_names() {
    let r = Repo::new();
    r.write("a.txt", "a\n");
    r.ok(&["one"]);
    let o = r.stone(&["adopt", "main"]);
    assert_eq!(o.code, 1);
    assert_eq!(o.stderr, "The stones of branch `main` are already here.\n");
    let o = r.stone(&["adopt", "refs/heads/main"]);
    assert_eq!(o.code, 1);
    let o = r.stone(&["adopt", "bad..name"]);
    assert_eq!(o.code, 1);
    assert!(o.stderr.contains("not a valid branch name"), "{}", o.stderr);
    r.git(&["checkout", "-q", "--detach"]);
    r.ok(&["adopt", "main"]);
    let o = r.stone(&["adopt", "detached"]);
    assert_eq!(o.code, 1);
    assert!(o.stderr.contains("already here"));
}

#[test]
fn a_branch_called_detached_is_reached_with_its_full_name() {
    let r = Repo::new();
    r.git(&["checkout", "-q", "-b", "detached"]);
    r.write("a.txt", "a\n");
    r.ok(&["odd"]);
    r.git(&["checkout", "-q", "main"]);
    // The bare word means the detached HEAD chain, which does not exist.
    assert_eq!(r.stone(&["adopt", "detached"]).code, 1);
    assert!(exists(&r, "refs/stones/heads/detached"));
    let out = r.ok(&["adopt", "refs/heads/detached"]);
    assert!(out.starts_with("Moved 1 stone from branch `detached` to branch `main`."));
    assert!(!exists(&r, "refs/stones/heads/detached"));
}

#[test]
fn the_normal_movement_rules_apply_right_after() {
    let r = detached_with_two_stones();
    r.git(&["switch", "-q", "-c", "feat"]);
    // The branch moves before the stones are adopted.
    r.git(&["commit", "-q", "--allow-empty", "-m", "elsewhere"]);
    r.ok(&["adopt", "detached"]);
    let status = r.ok(&["status"]);
    assert!(
        status.contains("moved since the stones were made"),
        "{status}"
    );
    let o = r.stone(&["third"]);
    assert_eq!(o.code, 1);
    assert!(o.stderr.contains("moved since the stones were made"));
    // Repair fixes it, as for any chain.
    assert_eq!(r.stone(&["repair", "--apply"]).code, 0);
    assert!(!r.ok(&["status"]).contains("moved"));
}
