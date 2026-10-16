mod common;

use common::Repo;

/// Three stones (a.txt, b.txt, c.txt named one, two, three) and a file of unstoned work.
fn three() -> Repo {
    let r = Repo::new();
    for (file, name) in [("a.txt", "one"), ("b.txt", "two"), ("c.txt", "three")] {
        r.write(file, &format!("{file}\n"));
        r.ok(&[name]);
    }
    r.write("unstoned.txt", "u\n");
    r
}

fn head_tree(r: &Repo) -> String {
    r.git(&["rev-parse", "HEAD^{tree}"]).trim().to_string()
}

fn head(r: &Repo) -> String {
    r.git(&["rev-parse", "HEAD"]).trim().to_string()
}

fn staged(r: &Repo) -> String {
    r.git(&["ls-files", "-s"])
}

fn state_oid(r: &Repo) -> String {
    r.git(&["rev-parse", "refs/stones/heads/main"])
}

#[test]
fn commit_all_three_stones_in_order() {
    let r = three();
    let trees: Vec<String> = (1..=3).map(|i| r.stone_tree("main", i)).collect();
    let files = r.files();
    let first_head = head(&r);

    for i in 1..=3usize {
        let o = r.stone(&[&i.to_string(), "commit", "-m", &format!("feat: part {i}")]);
        assert_eq!(o.code, 0, "{}{}", o.stdout, o.stderr);
        let short = r.git(&["rev-parse", "--short=7", "HEAD"]);
        let next = if i < 3 {
            format!("Stone {} is now next.\n", i + 1)
        } else {
            "No stones are left.\n".to_string()
        };
        // git's own "[main abc1234] ..." summary comes first, then ours.
        let ours = format!(
            "Committed Stone {i} as {}: feat: part {i}\n{next}",
            short.trim()
        );
        assert!(o.stdout.ends_with(&ours), "{}", o.stdout);
        assert_eq!(
            head_tree(&r),
            trees[i - 1],
            "commit {i} must have exactly the stone's tree"
        );
    }
    // Three ordinary commits on top of the base, with the identity from config.
    let log = r.git(&[
        "log",
        "--format=%an <%ae> | %cn | %s",
        &format!("{first_head}..HEAD"),
    ]);
    assert_eq!(
        log,
        "Test <test@example.com> | Test | feat: part 3\nTest <test@example.com> | Test | feat: part 2\nTest <test@example.com> | Test | feat: part 1\n"
    );
    assert_eq!(
        r.git(&["rev-list", "--count", &format!("{first_head}..HEAD")])
            .trim(),
        "3"
    );
    // Working tree untouched; only the unstoned work shows in git status.
    assert_eq!(r.files(), files);
    assert_eq!(r.git(&["status", "--porcelain"]), "?? unstoned.txt\n");
    assert_eq!(r.ok(&["list"]), "No stones yet.\n");
    assert!(r
        .git(&["cat-file", "-p", "refs/stones/heads/main:meta"])
        .contains(&format!("anchor {}", head(&r))));
    assert_eq!(r.temp_indexes(), Vec::<String>::new());
    r.git(&["fsck", "--no-dangling"]);
    // Making the next stone just works.
    let created = r.ok(&["after"]);
    assert!(
        created.starts_with("Stone 4 created: after") && created.contains("1 file changed"),
        "{created}"
    );
}

#[test]
fn only_the_first_stone_can_be_committed() {
    let r = three();
    let before = r.fingerprint();
    let state = state_oid(&r);
    let o = r.stone(&["2", "commit", "-m", "x"]);
    assert_eq!(o.code, 1);
    assert_eq!(
        o.stderr,
        "Commit Stone 1 first, or drop it to fold it into Stone 2.\n"
    );
    r.assert_untouched(&before);
    assert_eq!(state_oid(&r), state);
    // After dropping Stone 1, Stone 2 is first and may be committed.
    let t2 = r.stone_tree("main", 2);
    r.ok(&["drop", "1"]);
    assert_eq!(r.stone(&["commit", "2", "-m", "now"]).code, 0);
    assert_eq!(head_tree(&r), t2);
}

#[test]
fn staged_work_on_other_paths_survives_a_commit() {
    let r = three();
    r.write("staged.txt", "s\n");
    r.git(&["add", "staged.txt"]);
    r.write("a.txt", "a.txt\nand more\n"); // unstaged edit of a stoned file
    let o = r.stone(&["1", "commit", "-m", "one"]);
    assert_eq!(o.code, 0, "{}", o.stderr);
    assert_eq!(r.git(&["diff", "--cached", "--name-only"]), "staged.txt\n");
    assert!(r
        .git(&["status", "--porcelain"])
        .contains("?? unstoned.txt"));
    assert_eq!(r.read("a.txt"), "a.txt\nand more\n");
    assert_eq!(r.git(&["show", "HEAD:a.txt"]), "a.txt\n");
}

#[test]
fn staged_work_that_clashes_is_refused_before_anything_changes() {
    let r = three();
    r.write("a.txt", "a different version of the file\n");
    r.git(&["add", "a.txt"]); // Stone 1 also adds a.txt, with other content
    let before = r.fingerprint();
    let state = state_oid(&r);
    let o = r.stone(&["1", "commit", "-m", "one"]);
    assert_eq!(o.code, 1, "{}{}", o.stdout, o.stderr);
    assert!(
        o.stderr
            .contains("Your staged changes clash with Stone 1, so nothing was committed."),
        "{}",
        o.stderr
    );
    assert!(o.stderr.contains("git restore --staged"));
    r.assert_untouched(&before);
    assert_eq!(state_oid(&r), state);
    assert_eq!(r.temp_indexes(), Vec::<String>::new());
    // After unstaging, the same commit works.
    r.git(&["restore", "--staged", "a.txt"]);
    assert_eq!(r.stone(&["1", "commit", "-m", "one"]).code, 0);
}

#[test]
fn a_hook_that_refuses_changes_nothing() {
    let r = three();
    r.hook("commit-msg", "echo 'no way' >&2; exit 1");
    let before = r.fingerprint();
    let state = state_oid(&r);
    let o = r.stone(&["1", "commit", "-m", "one"]);
    assert_eq!(o.code, 1);
    assert!(
        o.stderr.contains("no way") && o.stderr.contains("Nothing was changed."),
        "{}",
        o.stderr
    );
    r.assert_untouched(&before);
    assert_eq!(state_oid(&r), state);
    assert_eq!(r.temp_indexes(), Vec::<String>::new());
    assert_eq!(r.ok(&["list"]), "  1  one\n  2  two\n  3  three\n");
}

#[test]
fn an_empty_message_changes_nothing() {
    let r = three();
    let before = r.fingerprint();
    let o = r.stone(&["1", "commit", "-m", ""]);
    assert_ne!(o.code, 0);
    r.assert_untouched(&before);
    assert_eq!(r.ok(&["list"]).lines().count(), 3);
}

#[test]
fn a_hook_that_adds_files_gets_a_warning_and_the_chain_stays_consistent() {
    let r = three();
    r.hook(
        "pre-commit",
        "[ -e extra.txt ] || { echo formatted > extra.txt; git add extra.txt; }",
    );
    let o = r.stone(&["1", "commit", "-m", "one"]);
    assert_eq!(o.code, 0, "{}", o.stderr);
    assert!(
        o.stderr.contains(
            "the commit is not exactly Stone 1; a hook changed these paths:\n  extra.txt"
        ),
        "{}",
        o.stderr
    );
    assert!(o
        .stderr
        .contains("The next stone will start from the commit as it was made."));
    // The commit holds the hook's file; the state moved on to the real commit.
    assert!(r
        .git(&["ls-tree", "-r", "--name-only", "HEAD"])
        .contains("extra.txt"));
    assert_eq!(r.ok(&["list"]), "  2  two\n  3  three\n");
    assert!(r
        .git(&["cat-file", "-p", "refs/stones/heads/main:meta"])
        .contains(&format!("anchor {}", head(&r))));
    // The next stone commits cleanly: its tree is exactly Stone 2.
    let t2 = r.stone_tree("main", 2);
    let o = r.stone(&["2", "commit", "-m", "two"]);
    assert_eq!(o.code, 0, "{}", o.stderr);
    assert_eq!(head_tree(&r), t2);
    r.git(&["fsck", "--no-dangling"]);
}

#[test]
fn signing_is_honoured() {
    if std::process::Command::new("ssh-keygen")
        .arg("-?")
        .output()
        .is_err()
    {
        eprintln!("ssh-keygen is not installed; skipping");
        return;
    }
    let r = three();
    let t1 = r.stone_tree("main", 1);
    let key = r.tmp.path().join("signing_key");
    let made = std::process::Command::new("ssh-keygen")
        .args(["-q", "-t", "ed25519", "-N", "", "-C", "test", "-f"])
        .arg(&key)
        .output()
        .unwrap();
    assert!(
        made.status.success(),
        "{}",
        String::from_utf8_lossy(&made.stderr)
    );
    r.git(&["config", "gpg.format", "ssh"]);
    r.git(&["config", "user.signingkey", key.to_str().unwrap()]);
    r.git(&["config", "commit.gpgSign", "true"]);
    let o = r.stone(&["1", "commit", "-m", "signed"]);
    assert_eq!(o.code, 0, "{}{}", o.stdout, o.stderr);
    let commit = r.git(&["cat-file", "-p", "HEAD"]);
    assert!(
        commit.contains("gpgsig -----BEGIN SSH SIGNATURE-----"),
        "{commit}"
    );
    assert_eq!(head_tree(&r), t1);
}

#[test]
fn author_date_signoff_and_message_files_are_passed_to_git() {
    let r = three();
    let o = r.stone(&[
        "1",
        "commit",
        "-m",
        "with options",
        "-s",
        "--author",
        "Ann Other <ann@example.com>",
        "--date",
        "2020-02-03T04:05:06Z",
    ]);
    assert_eq!(o.code, 0, "{}", o.stderr);
    let log = r.git(&["log", "-1", "--format=%an <%ae>|%aI|%cn|%B"]);
    assert!(
        log.starts_with("Ann Other <ann@example.com>|2020-02-03T04:05:06Z|Test|with options"),
        "{log}"
    );
    assert!(log.contains("Signed-off-by: Test <test@example.com>"));

    // A relative -F path means the path from where the user stands, not from the top of the repo.
    r.write("sub/msg.txt", "from a file\n");
    let o = r.stone_in(&r.dir.join("sub"), &["commit", "2", "-F", "msg.txt"]);
    assert_eq!(o.code, 0, "{}", o.stderr);
    assert_eq!(r.git(&["log", "-1", "--format=%s"]), "from a file\n");
}

#[test]
fn options_outside_the_allowed_list_are_refused() {
    let r = three();
    let before = r.fingerprint();
    for args in [
        vec!["--amend"],
        vec!["-a", "-m", "x"],
        vec!["--allow-empty", "-m", "x"],
        vec!["-m"],
    ] {
        let mut full = vec!["1", "commit"];
        full.extend(args.iter());
        let o = r.stone(&full);
        assert_eq!(o.code, 1, "{args:?}");
        assert!(
            o.stderr.contains("does not support") || o.stderr.contains("needs a value"),
            "{}",
            o.stderr
        );
    }
    r.assert_untouched(&before);
    assert_eq!(r.ok(&["list"]).lines().count(), 3);
}

#[test]
fn a_locked_index_is_refused_up_front() {
    let r = three();
    std::fs::write(r.dir.join(".git/index.lock"), "").unwrap();
    let before = r.fingerprint();
    let head_before = head(&r);
    let o = r.stone(&["1", "commit", "-m", "x"]);
    assert_eq!(o.code, 1);
    assert!(
        o.stderr.contains("The staging area is locked"),
        "{}",
        o.stderr
    );
    assert_eq!(head(&r), head_before);
    r.assert_untouched(&before);
    assert_eq!(r.temp_indexes(), Vec::<String>::new());
}

#[test]
fn commit_and_create_refuse_during_a_merge() {
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
    r.write("a.txt", "a\n");
    r.ok(&["a"]);
    assert_ne!(r.git_try(&["merge", "topic"]).code, 0);
    let o = r.stone(&["1", "commit", "-m", "x"]);
    assert_eq!(o.code, 1);
    assert!(
        o.stderr
            .contains("A merge is in progress. Finish or abort it before committing."),
        "{}",
        o.stderr
    );
}

#[test]
fn commit_all_commits_every_stone_with_its_name() {
    let r = three();
    let trees: Vec<String> = (1..=3).map(|i| r.stone_tree("main", i)).collect();
    let base = head(&r);
    let o = r.stone(&["commit-all"]);
    assert_eq!(o.code, 0, "{}{}", o.stdout, o.stderr);
    assert_eq!(
        r.git(&["log", "--format=%s", &format!("{base}..HEAD")]),
        "three\ntwo\none\n"
    );
    assert_eq!(head_tree(&r), trees[2]);
    assert_eq!(r.git(&["rev-parse", "HEAD~1^{tree}"]).trim(), trees[1]);
    assert_eq!(r.ok(&["list"]), "No stones yet.\n");
    assert_eq!(r.git(&["status", "--porcelain"]), "?? unstoned.txt\n");
    assert!(o.stdout.contains("Committed Stone 1 as") && o.stdout.contains("No stones are left."));
    // Nothing left to commit is an error.
    assert_eq!(r.stone(&["commit-all"]).code, 1);
}

#[test]
fn commit_all_stops_at_a_failing_hook_and_keeps_what_was_done() {
    let r = three();
    r.hook(
        "commit-msg",
        "if grep -q '^two$' \"$1\"; then echo 'two is not allowed' >&2; exit 1; fi",
    );
    let base = head(&r);
    let t1 = r.stone_tree("main", 1);
    let o = r.stone(&["commit-all"]);
    assert_eq!(o.code, 1, "{}{}", o.stdout, o.stderr);
    assert!(o.stderr.contains("two is not allowed"));
    // Stone 1 is committed and recorded; Stones 2 and 3 remain, still based on the new commit.
    assert_eq!(
        r.git(&["log", "--format=%s", &format!("{base}..HEAD")]),
        "one\n"
    );
    assert_eq!(head_tree(&r), t1);
    assert_eq!(r.ok(&["list"]), "  2  two\n  3  three\n");
    assert!(r
        .git(&["cat-file", "-p", "refs/stones/heads/main:meta"])
        .contains(&format!("anchor {}", head(&r))));
    assert!(r
        .git(&["status", "--porcelain"])
        .contains("?? unstoned.txt"));
    // Fix the hook and carry on.
    r.hook("commit-msg", "exit 0");
    assert_eq!(r.stone(&["commit-all"]).code, 0);
    assert_eq!(r.ok(&["list"]), "No stones yet.\n");
}

#[test]
fn unborn_branch_first_commit_is_a_root_commit() {
    let r = Repo::unborn();
    r.write("first.txt", "1\n");
    r.ok(&["first"]);
    r.write("second.txt", "2\n");
    r.ok(&["second"]);
    let t1 = r.stone_tree("main", 1);
    let o = r.stone(&["1", "commit", "-m", "root"]);
    assert_eq!(o.code, 0, "{}{}", o.stdout, o.stderr);
    assert_eq!(
        r.git(&["rev-list", "--parents", "-n1", "HEAD"]).trim(),
        head(&r),
        "a root commit has no parent"
    );
    assert_eq!(head_tree(&r), t1);
    assert_eq!(r.ok(&["list"]), "  2  second\n");
    // Stone 2 now sits on the new commit.
    let diff = r.stone(&["diff", "2"]).stdout;
    assert!(
        diff.contains("+++ b/second.txt") && !diff.contains("first.txt"),
        "{diff}"
    );
    let o = r.stone(&["2", "commit", "-m", "child"]);
    assert_eq!(o.code, 0, "{}", o.stderr);
    assert_eq!(r.git(&["rev-list", "--count", "HEAD"]).trim(), "2");
    assert_eq!(r.git(&["status", "--porcelain"]), "");
}

#[test]
fn detached_head_moves_and_the_chain_follows() {
    let r = Repo::new();
    r.git(&["checkout", "-q", "--detach"]);
    r.write("a.txt", "a\n");
    r.ok(&["a"]);
    r.write("b.txt", "b\n");
    r.ok(&["b"]);
    let before = head(&r);
    assert_eq!(r.stone(&["1", "commit", "-m", "detached commit"]).code, 0);
    assert_ne!(head(&r), before);
    assert_eq!(r.git(&["rev-parse", "HEAD~1"]).trim(), before);
    assert_eq!(
        r.git_try(&["symbolic-ref", "-q", "HEAD"]).code,
        1,
        "still detached"
    );
    assert_eq!(r.ok(&["list"]), "  2  b\n");
    assert!(r
        .git(&["cat-file", "-p", "refs/stones/detached/main:meta"])
        .contains(&format!("anchor {}", head(&r))));
    assert_eq!(r.stone(&["2", "commit", "-m", "again"]).code, 0);
}

/// Two identical repos with two stones, one staged file and some unstoned work.
fn twin() -> Repo {
    let r = Repo::new();
    r.write("a.txt", "a\n");
    r.ok(&["one"]);
    r.write("b.txt", "b\n");
    r.ok(&["two"]);
    r.write("staged.txt", "s\n");
    r.git(&["add", "staged.txt"]);
    r.write("unstoned.txt", "u\n");
    r
}

#[test]
fn an_interrupted_commit_is_found_and_healed_by_the_next_mutating_command() {
    for point in ["commit:after-git-commit", "commit:after-index-update"] {
        // The reference: an uninterrupted run of the same steps.
        let clean = twin();
        assert_eq!(clean.stone(&["1", "commit", "-m", "one"]).code, 0);

        let broken = twin();
        let o = broken.stone_env(
            &broken.dir,
            &["1", "commit", "-m", "one"],
            &[("GIT_STONE_FAILPOINT", point)],
        );
        assert_eq!(o.code, 99, "{point}: {}{}", o.stdout, o.stderr);
        // The commit exists but the stones do not know yet.
        assert_eq!(head_tree(&broken), broken.stone_tree("main", 1));
        assert_eq!(broken.ok(&["list"]), "  1  one\n  2  two\n", "{point}");
        let status = broken.stone(&["status"]);
        assert!(
            status
                .stdout
                .contains("Stone 1 already on the branch as commits"),
            "{point}: {}",
            status.stdout
        );
        assert!(status.stderr.is_empty() || status.stderr.contains("already on the branch"));
        let porcelain = broken.ok(&["status", "--porcelain"]);
        assert!(
            porcelain.contains("warning\tstones-already-committed\t"),
            "{porcelain}"
        );

        // The next mutating command (here: drop) marks Stone 1 committed and fixes the index.
        let o = broken.stone(&["drop", "2"]);
        assert_eq!(o.code, 0, "{point}: {}", o.stderr);
        assert!(
            o.stderr.contains(
                "Note: Stone 1 matches a commit already on the branch; marked as committed."
            ),
            "{}",
            o.stderr
        );
        clean.ok(&["drop", "2"]);

        assert_eq!(head(&broken), head(&clean), "{point}: same commit");
        assert_eq!(
            staged(&broken),
            staged(&clean),
            "{point}: same staged state as an uninterrupted run"
        );
        assert_eq!(
            broken.git(&["status", "--porcelain"]),
            clean.git(&["status", "--porcelain"])
        );
        assert_eq!(broken.ok(&["list"]), clean.ok(&["list"]));
        assert!(broken
            .git(&["cat-file", "-p", "refs/stones/heads/main:meta"])
            .contains(&format!("anchor {}", head(&broken))));
        broken.git(&["fsck", "--no-dangling"]);
    }
}

#[test]
fn committing_the_next_stone_also_heals_an_interrupted_commit() {
    let clean = twin();
    clean.ok(&["1", "commit", "-m", "one"]);
    clean.ok(&["2", "commit", "-m", "two"]);

    let broken = twin();
    let o = broken.stone_env(
        &broken.dir,
        &["1", "commit", "-m", "one"],
        &[("GIT_STONE_FAILPOINT", "commit:after-git-commit")],
    );
    assert_eq!(o.code, 99);
    let o = broken.stone(&["2", "commit", "-m", "two"]);
    assert_eq!(o.code, 0, "{}", o.stderr);
    assert!(o.stderr.contains("marked as committed"));
    assert_eq!(head(&broken), head(&clean));
    assert_eq!(staged(&broken), staged(&clean));
    assert_eq!(broken.ok(&["list"]), "No stones yet.\n");
}

#[test]
fn a_hand_made_commit_that_equals_the_first_stones_is_recognised() {
    let r = twin();
    // The user commits exactly Stone 1's tree by hand (index = Stone 1, then a plain commit).
    r.git(&["read-tree", &r.stone_tree("main", 1)]);
    r.git(&["commit", "-q", "-m", "by hand"]);
    assert_eq!(head_tree(&r), r.stone_tree("main", 1));
    let t2 = r.stone_tree("main", 2);
    let o = r.stone(&["2", "commit", "-m", "two"]);
    assert_eq!(o.code, 0, "{}", o.stderr);
    assert!(
        o.stderr
            .contains("Note: Stone 1 matches a commit already on the branch; marked as committed."),
        "{}",
        o.stderr
    );
    assert_eq!(head_tree(&r), t2);
    assert_eq!(r.git(&["log", "--format=%s", "-3"]), "two\nby hand\nbase\n");
    assert_eq!(r.ok(&["list"]), "No stones yet.\n");
}

#[test]
fn two_hand_made_commits_are_recognised_together_and_create_continues() {
    let r = twin();
    for i in 1..=2 {
        r.git(&["read-tree", &r.stone_tree("main", i)]);
        r.git(&["commit", "-q", "-m", &format!("hand {i}")]);
    }
    let o = r.stone(&["after both"]);
    // Nothing differs from the commits except the unstoned files, which are new work.
    assert_eq!(o.code, 0, "{}{}", o.stdout, o.stderr);
    assert!(
        o.stderr
            .contains("Note: Stones 1–2 match commits already on the branch; marked as committed."),
        "{}",
        o.stderr
    );
    assert!(
        o.stdout.starts_with("Stone 3 created: after both"),
        "{}",
        o.stdout
    );
    assert_eq!(r.ok(&["list"]), "  3  after both\n");
}

#[test]
fn an_unborn_branch_commit_by_hand_is_recognised() {
    let r = Repo::unborn();
    r.write("a.txt", "a\n");
    r.ok(&["one"]);
    r.git(&["add", "-A"]);
    r.git(&["commit", "-q", "-m", "root by hand"]);
    assert!(r
        .ok(&["status", "--porcelain"])
        .contains("warning\tstones-already-committed\t"));
    r.write("b.txt", "b\n");
    let o = r.stone(&["two"]);
    assert_eq!(o.code, 0, "{}", o.stderr);
    assert!(
        o.stderr.contains("Note: Stone 1 matches a commit"),
        "{}",
        o.stderr
    );
    assert_eq!(r.ok(&["list"]), "  2  two\n");
}

#[test]
fn other_movements_are_not_reconciled() {
    // A matching commit followed by another commit: more commits than stones can match.
    let r = Repo::new();
    r.write("a.txt", "a\n");
    r.ok(&["one"]);
    r.git(&["add", "-A"]);
    r.git(&["commit", "-q", "-m", "matches"]);
    r.git(&["commit", "-q", "--allow-empty", "-m", "and one more"]);
    assert!(r
        .ok(&["status", "--porcelain"])
        .contains("warning\tbranch-moved\t"));
    let o = r.stone(&["1", "commit", "-m", "x"]);
    assert_eq!(o.code, 1);
    assert!(
        o.stderr.contains("moved since the stones were made"),
        "{}",
        o.stderr
    );

    // Reset behind the anchor: the anchor is not an ancestor any more.
    let r = Repo::new();
    r.write("a.txt", "a\n");
    r.ok(&["one"]);
    r.git(&["reset", "-q", "--hard", "HEAD~0"]);
    r.git(&["commit", "-q", "--allow-empty", "-m", "extra"]);
    r.git(&["reset", "-q", "--hard", "HEAD~1"]);
    r.git(&["commit", "-q", "--allow-empty", "-m", "different"]);
    assert!(r
        .ok(&["status", "--porcelain"])
        .contains("warning\tbranch-moved\t"));

    // An ordinary commit with other content.
    let r = Repo::new();
    r.write("a.txt", "a\n");
    r.ok(&["one"]);
    r.write("other.txt", "o\n");
    r.git(&["add", "other.txt"]);
    r.git(&["commit", "-q", "-m", "ordinary"]);
    let before = r.fingerprint();
    for args in [vec!["x"], vec!["drop", "1"], vec!["1", "commit", "-m", "x"]] {
        let o = r.stone(&args);
        assert_eq!(o.code, 1, "{args:?}");
        assert!(
            o.stderr
                .contains("Nothing was changed. Run `git stone repair`"),
            "{}",
            o.stderr
        );
    }
    r.assert_untouched(&before);
}

#[test]
fn a_branch_that_moves_during_the_commit_is_detected() {
    let r = three();
    r.hook(
        "post-commit",
        "[ -e ../raced ] && exit 0\ntouch ../raced\nunset GIT_INDEX_FILE\ngit commit -q --allow-empty -m raced",
    );
    // `../raced` is relative to the repo's top, where hooks run.
    let state = state_oid(&r);
    let staged_before = staged(&r);
    let o = r.stone(&["1", "commit", "-m", "one"]);
    assert_eq!(o.code, 1, "{}{}", o.stdout, o.stderr);
    assert!(
        o.stderr
            .contains("the branch also moved for another reason"),
        "{}",
        o.stderr
    );
    // Neither the index nor the stones were touched after the commit.
    assert_eq!(state_oid(&r), state);
    assert_eq!(staged(&r), staged_before);
    assert!(r
        .ok(&["status", "--porcelain"])
        .contains("warning\tbranch-moved\t"));
}

#[test]
fn nothing_to_commit_in_an_empty_stone_changes_nothing() {
    let r = Repo::new();
    r.ok(&["marker", "--allow-empty"]);
    let before = r.fingerprint();
    let o = r.stone(&["1", "commit", "-m", "x"]);
    assert_ne!(o.code, 0);
    assert!(o.stderr.contains("Nothing was changed."), "{}", o.stderr);
    r.assert_untouched(&before);
    assert_eq!(r.ok(&["list"]), "  1  marker\n");
}

/// The main use: Stone 2 edits a file Stone 1 created. The index entry written by the first commit has
/// no file stamps, so a merge that checks the working tree would call it "not uptodate" (design.md, Phase 5).
#[test]
fn a_stone_that_builds_on_an_earlier_stone_commits_cleanly() {
    let r = Repo::new();
    r.write("auth.txt", "login\n");
    r.ok(&["auth"]);
    r.write("auth.txt", "login\nrefresh\n");
    r.ok(&["authorization"]);
    r.write("auth.txt", "login\nrefresh\nlogout\n"); // more unstoned work on the same file
    r.write("staged.txt", "s\n");
    r.git(&["add", "staged.txt"]);

    let t2 = r.stone_tree("main", 2);
    assert_eq!(r.stone(&["1", "commit", "-m", "auth"]).code, 0);
    let o = r.stone(&["2", "commit", "-m", "authorization"]);
    assert_eq!(o.code, 0, "{}{}", o.stdout, o.stderr);
    assert_eq!(head_tree(&r), t2);
    assert_eq!(r.git(&["show", "HEAD:auth.txt"]), "login\nrefresh\n");
    // The unstoned third line and the staged file are exactly as the user left them.
    assert_eq!(r.read("auth.txt"), "login\nrefresh\nlogout\n");
    assert_eq!(r.git(&["diff", "--cached", "--name-only"]), "staged.txt\n");
    assert_eq!(r.git(&["diff", "--name-only"]), "auth.txt\n");
}
