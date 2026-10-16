mod common;

use common::Repo;

#[test]
fn outside_a_repository() {
    let tmp = tempfile::tempdir().unwrap();
    let r = Repo::new();
    let o = r.stone_in(tmp.path(), &["status"]);
    assert_eq!(o.code, 1);
    assert!(o.stderr.contains("not a git repository"), "{}", o.stderr);
}

#[test]
fn bare_repository() {
    let tmp = tempfile::tempdir().unwrap();
    let r = Repo::new();
    let bare = tmp.path().join("bare.git");
    std::fs::create_dir(&bare).unwrap();
    r.git_in(&bare, &["init", "-q", "--bare"]);
    let o = r.stone_in(&bare, &["list"]);
    assert_eq!(o.code, 1);
    assert!(o.stderr.contains("bare repository"), "{}", o.stderr);
}

#[test]
fn nothing_changed() {
    let r = Repo::new();
    let o = r.stone(&["empty"]);
    assert_eq!(
        (o.code, o.stderr.as_str()),
        (1, "Nothing changed since the branch commit.\n")
    );
    r.write("a.txt", "a\n");
    r.ok(&["a"]);
    let o = r.stone(&["again"]);
    assert_eq!(
        (o.code, o.stderr.as_str()),
        (1, "Nothing changed since Stone 1.\n")
    );
    // --allow-empty overrides, and the new stone is an empty boundary.
    let o = r.stone(&["marker", "--allow-empty"]);
    assert_eq!(o.code, 0, "{}", o.stderr);
    assert!(o.stdout.contains("Stone 2 created: marker") && o.stdout.contains("no changes"));
    assert_eq!(r.stone_tree("main", 1), r.stone_tree("main", 2));
}

#[test]
fn unborn_and_empty_working_tree() {
    let r = Repo::unborn();
    let o = r.stone(&["nothing"]);
    assert_eq!(o.code, 1);
    assert!(o.stderr.contains("working tree is empty"), "{}", o.stderr);
}

#[test]
fn working_tree_equal_to_the_branch_commit_after_a_stash() {
    let r = Repo::new();
    r.write("a.txt", "a\n");
    r.ok(&["a"]);
    r.git(&["stash", "-u", "-q"]);
    let o = r.stone(&["after stash"]);
    assert_eq!(o.code, 1);
    assert!(
        o.stderr.contains("Did you stash? Use --force"),
        "{}",
        o.stderr
    );
    assert!(!r.state_ref_exists("refs/stones/heads/main:trees/2"));
    let o = r.stone(&["after stash", "--force"]);
    assert_eq!(o.code, 0, "{}", o.stderr);
    assert_eq!(
        r.stone_tree("main", 2),
        r.git(&["rev-parse", "HEAD^{tree}"]).trim()
    );
}

fn conflicted_merge(r: &Repo) {
    r.write("f.txt", "base\n");
    r.git(&["add", "f.txt"]);
    r.git(&["commit", "-q", "-m", "f"]);
    r.git(&["checkout", "-q", "-b", "topic"]);
    r.write("f.txt", "topic\n");
    r.git(&["commit", "-q", "-am", "topic"]);
    r.git(&["checkout", "-q", "main"]);
    r.write("f.txt", "main\n");
    r.git(&["commit", "-q", "-am", "main"]);
    assert_ne!(r.git_try(&["merge", "topic"]).code, 0);
}

#[test]
fn merge_in_progress_and_unmerged_paths() {
    let r = Repo::new();
    conflicted_merge(&r);
    let before = r.fingerprint();
    let o = r.stone(&["during merge"]);
    assert_eq!(o.code, 1);
    assert!(o.stderr.contains("A merge is in progress"), "{}", o.stderr);
    r.assert_untouched(&before);
    assert!(!r.state_ref_exists("refs/stones/heads/main"));
    // Read commands still work and say what is wrong.
    let status = r.ok(&["status"]);
    assert!(
        status.contains("A merge is in progress.") && status.contains("merge conflicts"),
        "{status}"
    );

    // Resolved in the index but MERGE_HEAD still there: the operation check fires first, and
    // with MERGE_HEAD gone but a conflict left in the index, the unmerged check does.
    r.git(&["merge", "--abort"]);
    r.write("g.txt", "x\n");
    r.git(&["add", "g.txt"]);
    let blob = r.git(&["hash-object", "-w", "f.txt"]);
    let info = format!(
        "100644 {} 1\tf.txt\n100644 {} 2\tf.txt\n",
        blob.trim(),
        blob.trim()
    );
    let mut child = std::process::Command::new("git")
        .args(["update-index", "--index-info"])
        .current_dir(&r.dir)
        .stdin(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    use std::io::Write;
    child
        .stdin
        .take()
        .unwrap()
        .write_all(info.as_bytes())
        .unwrap();
    assert!(child.wait().unwrap().success());
    let o = r.stone(&["conflict markers"]);
    assert_eq!(o.code, 1);
    assert!(o.stderr.contains("merge conflicts"), "{}", o.stderr);
}

#[test]
fn bad_ids_and_lone_numbers() {
    let r = Repo::new();
    let o = r.stone(&["show", "7"]);
    assert_eq!(o.code, 1);
    assert!(
        o.stderr
            .contains("There is no Stone 7: no stones have been made here yet."),
        "{}",
        o.stderr
    );
    r.write("a.txt", "a\n");
    r.ok(&["a"]);
    for args in [
        vec!["show", "7"],
        vec!["diff", "7"],
        vec!["7", "diff"],
        vec!["7", "show"],
    ] {
        let o = r.stone(&args);
        assert_eq!(o.code, 1, "{args:?}");
        assert!(
            o.stderr
                .contains("There is no Stone 7. Run `git stone list`"),
            "{}",
            o.stderr
        );
    }
    let o = r.stone(&["42"]);
    assert_eq!(o.code, 1);
    assert_eq!(o.stderr, "git-stone: Did you mean `git stone show 42`? To create a stone named 42, use `git stone -- 42`.\n");
    assert_eq!(r.stone(&["show", "abc"]).code, 1);
}

#[test]
fn control_characters_and_bad_sizes_are_refused() {
    let r = Repo::new();
    r.write("a.txt", "a\n");
    let before = r.fingerprint();
    for name in ["new\nline", "tab\there", "esc\u{1b}[0m", "", "   "] {
        let o = r.stone(&[name]);
        assert_eq!(o.code, 1, "{name:?}");
    }
    assert_eq!(r.stone(&[&"x".repeat(201)]).code, 1);
    assert_eq!(r.stone(&["ok", "-d", &"x".repeat(10_001)]).code, 1);
    r.assert_untouched(&before);
    assert!(!r.state_ref_exists("refs/stones/heads/main"));
}

#[test]
fn creating_after_the_branch_moved_is_refused_with_the_full_message() {
    let r = Repo::new();
    r.write("a.txt", "a\n");
    r.ok(&["a"]);
    let anchor = r.git(&["rev-parse", "--short=7", "HEAD"]);
    r.git(&["commit", "-q", "--allow-empty", "-m", "someone else"]);
    let now = r.git(&["rev-parse", "--short=7", "HEAD"]);
    r.write("b.txt", "b\n");
    let before = r.fingerprint();
    let o = r.stone(&["b"]);
    assert_eq!(o.code, 1);
    assert_eq!(
        o.stderr,
        format!(
            "Git Stone: branch `main` moved since the stones were made.\n  Stones are based on commit {}.\n  The branch is now at   {}.\nNothing was changed. Run `git stone repair` to see how to fix this.\n",
            anchor.trim(),
            now.trim()
        )
    );
    r.assert_untouched(&before);
}

#[test]
fn commands_not_built_yet_say_so() {
    let r = Repo::new();
    let o = r.stone(&["adopt", "detached"]);
    assert_eq!(o.code, 2);
    assert!(o.stderr.contains("not implemented yet"));
}
