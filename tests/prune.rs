mod common;

use std::fs;
use std::time::{Duration, SystemTime};

use common::Repo;

/// A branch `old` whose chain is empty (its only stone was dropped), then the branch is deleted.
fn deleted_branch(keep_stone: bool) -> Repo {
    let r = Repo::new();
    r.git(&["checkout", "-q", "-b", "old"]);
    r.write("a.txt", "a\n");
    r.ok(&["first"]);
    if !keep_stone {
        r.ok(&["1", "drop"]);
    }
    r.git(&["checkout", "-q", "main"]);
    r.git(&["branch", "-D", "old"]);
    r
}

fn exists(r: &Repo, name: &str) -> bool {
    r.state_ref_exists(name)
}

#[test]
fn a_repo_without_leftovers_has_nothing_to_clean() {
    let r = Repo::new();
    assert_eq!(r.ok(&["prune"]), "Nothing to clean up.\n");
    r.write("a.txt", "a\n");
    r.ok(&["work"]);
    r.git(&["checkout", "-q", "-b", "other"]);
    // Chains of branches that exist are never leftovers, empty or not.
    r.ok(&["prune", "--apply"]);
    assert!(exists(&r, "refs/stones/heads/main"));
}

#[test]
fn the_plan_changes_nothing_and_apply_removes_empty_chains_of_deleted_branches() {
    let r = deleted_branch(false);
    let before = r.fingerprint();
    let chain = r
        .git(&["rev-parse", "refs/stones/heads/old"])
        .trim()
        .to_string();

    let plan = r.ok(&["prune"]);
    println!("{plan}");
    assert!(
        plan.starts_with("Would remove:\n  branch `old` (no stones)\n"),
        "{plan}"
    );
    assert!(plan.contains("Run `git stone prune --apply` to do it."));
    assert!(exists(&r, "refs/stones/heads/old"));
    r.assert_untouched(&before);

    let done = r.ok(&["prune", "--apply"]);
    println!("{done}");
    assert!(
        done.starts_with("Removed:\n  branch `old` (no stones)\n"),
        "{done}"
    );
    assert!(done.contains(&format!("git update-ref refs/stones/heads/old {chain}")));
    assert!(!exists(&r, "refs/stones/heads/old"));
    r.assert_untouched(&before);
    r.git(&["fsck", "--no-dangling"]);
    assert_eq!(r.ok(&["prune"]), "Nothing to clean up.\n");

    // The printed command really does bring it back.
    r.git(&["update-ref", "refs/stones/heads/old", &chain]);
    assert!(r.ok(&["prune"]).contains("branch `old`"));
}

#[test]
fn chains_with_stones_are_kept_until_named() {
    let r = deleted_branch(true);
    let state = r.git(&["rev-parse", "refs/stones/heads/old"]);

    let plan = r.ok(&["prune"]);
    println!("{plan}");
    assert!(!plan.contains("Would remove"), "{plan}");
    assert!(plan.contains("Kept, because they still hold stones:"));
    assert!(plan.contains("branch `old` (1 stone: first)"));
    assert!(plan.contains("git stone prune --chain refs/stones/heads/old --apply"));
    assert!(plan.contains("git stone adopt old"));
    r.ok(&["prune", "--apply"]);
    assert_eq!(r.git(&["rev-parse", "refs/stones/heads/old"]), state);

    // Naming it without --apply is only a plan.
    let named = r.ok(&["prune", "--chain", "refs/stones/heads/old"]);
    assert!(
        named.starts_with("Would remove:\n  branch `old` (1 stone: first)"),
        "{named}"
    );
    assert!(named.contains("--chain refs/stones/heads/old --apply"));
    assert_eq!(r.git(&["rev-parse", "refs/stones/heads/old"]), state);

    let done = r.ok(&["prune", "--chain", "refs/stones/heads/old", "--apply"]);
    assert!(
        done.starts_with("Removed:\n  branch `old` (1 stone: first)"),
        "{done}"
    );
    assert!(!exists(&r, "refs/stones/heads/old"));
    r.git(&["fsck", "--no-dangling"]);
}

#[test]
fn a_chain_can_be_adopted_instead_of_pruned() {
    let r = deleted_branch(true);
    r.git(&["checkout", "-q", "-b", "new"]);
    r.ok(&["adopt", "old"]);
    assert_eq!(r.ok(&["prune"]), "Nothing to clean up.\n");
}

#[test]
fn bad_targets_are_refused() {
    let r = deleted_branch(true);
    r.write("a.txt", "a\n");
    r.ok(&["work"]);
    let before = r.git(&["for-each-ref", "refs/stones"]);
    for (target, needle) in [
        ("main", "is not a stone chain"),
        ("refs/heads/main", "is not a stone chain"),
        ("refs/stones/heads/nosuch", "There is no chain"),
        (
            "refs/stones/heads/main",
            "still belongs to a branch or worktree that exists",
        ),
    ] {
        let o = r.stone(&["prune", "--chain", target, "--apply"]);
        assert_eq!(o.code, 1, "{target}: {}{}", o.stdout, o.stderr);
        assert!(o.stderr.contains(needle), "{target}: {}", o.stderr);
    }
    assert_eq!(r.git(&["for-each-ref", "refs/stones"]), before);
    for args in [
        &["prune", "--chain"][..],
        &["prune", "--bogus"],
        &["prune", "x"],
    ] {
        assert_eq!(r.stone(args).code, 1, "{args:?}");
    }
}

#[test]
fn chains_of_removed_worktrees_are_leftovers_and_live_ones_are_not() {
    let r = Repo::new();
    let live = r.tmp.path().join("live");
    let gone = r.tmp.path().join("gone");
    for path in [&live, &gone] {
        r.git(&["worktree", "add", "-q", "--detach", path.to_str().unwrap()]);
        fs::write(path.join("t.txt"), "t\n").unwrap();
        assert_eq!(r.stone_in(path, &["twin"]).code, 0);
    }
    // `live` keeps its stone, so even a worktree that exists is never listed.
    r.stone_in(&gone, &["1", "drop"]);
    assert_eq!(r.ok(&["prune"]), "Nothing to clean up.\n");

    r.git(&["worktree", "remove", "--force", gone.to_str().unwrap()]);
    let plan = r.ok(&["prune"]);
    println!("{plan}");
    assert!(
        plan.contains("detached chain of removed worktree `linked/gone` (no stones)"),
        "{plan}"
    );
    assert!(!plan.contains("linked/live"));
    r.ok(&["prune", "--apply"]);
    assert!(!exists(&r, "refs/stones/detached/linked/gone"));
    assert!(exists(&r, "refs/stones/detached/linked/live"));

    // A removed worktree whose chain still has stones is kept.
    r.git(&["worktree", "remove", "--force", live.to_str().unwrap()]);
    let plan = r.ok(&["prune"]);
    assert!(
        plan.contains("Kept, because they still hold stones:"),
        "{plan}"
    );
    assert!(plan.contains("(1 stone: twin)"));
    assert!(
        !plan.contains("adopt"),
        "adopt only works for branch chains: {plan}"
    );
    r.ok(&[
        "prune",
        "--chain",
        "refs/stones/detached/linked/live",
        "--apply",
    ]);
    assert!(!exists(&r, "refs/stones/detached/linked/live"));
    r.git(&["fsck", "--no-dangling"]);
}

#[test]
fn the_chain_you_are_working_on_is_never_listed() {
    let r = Repo::new();
    r.git(&["checkout", "-q", "--detach"]);
    r.write("a.txt", "a\n");
    r.ok(&["detached"]);
    r.ok(&["1", "drop"]);
    // Empty, but it belongs to this worktree.
    assert_eq!(r.ok(&["prune"]), "Nothing to clean up.\n");
    // An unborn branch has no `refs/heads/...` yet and is still not a leftover.
    let r = Repo::unborn();
    r.write("a.txt", "a\n");
    r.ok(&["first"]);
    r.ok(&["1", "drop"]);
    assert_eq!(r.ok(&["prune", "--apply"]), "Nothing to clean up.\n");
    assert!(exists(&r, "refs/stones/heads/main"));
}

#[test]
fn old_temp_indexes_are_removed_and_young_ones_stay() {
    let r = Repo::new();
    let old = r.dir.join(".git/stone-index-1-2-3");
    let young = r.dir.join(".git/stone-index-4-5-6");
    for path in [&old, &young] {
        fs::write(path, "junk").unwrap();
    }
    fs::File::options()
        .write(true)
        .open(&old)
        .unwrap()
        .set_modified(SystemTime::now() - Duration::from_secs(2 * 3600))
        .unwrap();
    let plan = r.ok(&["prune"]);
    assert!(plan.contains("1 leftover temp index file"), "{plan}");
    assert!(old.exists());
    let done = r.ok(&["prune", "--apply"]);
    assert!(done.contains("1 leftover temp index file"));
    assert!(!old.exists() && young.exists());
}

#[test]
fn a_damaged_leftover_is_kept_and_only_removed_by_name() {
    let r = Repo::new();
    // A chain for a branch that does not exist, pointing at an ordinary commit.
    r.git(&["update-ref", "refs/stones/heads/broken", "HEAD"]);
    let plan = r.ok(&["prune", "--apply"]);
    println!("{plan}");
    assert!(
        plan.contains("Kept, because they still hold stones:"),
        "{plan}"
    );
    assert!(plan.contains("branch `broken` (cannot be read:"), "{plan}");
    assert!(exists(&r, "refs/stones/heads/broken"));
    r.ok(&["prune", "--chain", "refs/stones/heads/broken", "--apply"]);
    assert!(!exists(&r, "refs/stones/heads/broken"));
}
