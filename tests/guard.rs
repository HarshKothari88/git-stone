mod common;

use common::Repo;

fn hook(r: &Repo) -> std::path::PathBuf {
    r.dir.join(".git/hooks/reference-transaction")
}

/// A repo with one stone and a staged file ready for a plain commit.
fn with_a_stone_and_a_staged_file() -> Repo {
    let r = Repo::new();
    r.write("a.txt", "a\n");
    r.ok(&["one"]);
    r.write("b.txt", "b\n");
    r.git(&["add", "b.txt"]);
    r
}

fn head(r: &Repo) -> String {
    r.git(&["rev-parse", "HEAD"]).trim().to_string()
}

#[test]
fn block_refuses_plain_git_commands_that_move_the_branch_while_stones_exist() {
    let r = with_a_stone_and_a_staged_file();
    let out = r.ok(&["guard", "block"]);
    assert!(
        out.contains("Guard is now `block`") && out.contains("Hook installed"),
        "{out}"
    );
    assert!(hook(&r).exists());

    let before = head(&r);
    for args in [
        &["commit", "-q", "-m", "plain"][..],
        &["commit", "-q", "-m", "plain", "--no-verify"],
        &["commit", "-q", "--amend", "-m", "amended"],
        &["commit", "-q", "--allow-empty", "-m", "empty"][..],
    ] {
        let o = r.git_try(args);
        assert_ne!(o.code, 0, "{args:?} should be refused");
        assert!(
            o.stderr
                .contains("Git Stone guard: branch `main` has 1 stone"),
            "{}",
            o.stderr
        );
        assert!(o.stderr.contains("git stone commit-all"), "{}", o.stderr);
    }
    assert_eq!(head(&r), before, "the branch must not move");
    // The refusal changed nothing: the staged file is still staged.
    assert_eq!(r.git(&["diff", "--cached", "--name-only"]).trim(), "b.txt");
}

#[test]
fn block_lets_git_stone_itself_and_everything_that_is_not_a_branch_move_through() {
    let r = with_a_stone_and_a_staged_file();
    r.ok(&["guard", "block"]);
    // Making a stone writes refs/stones/..., not the branch.
    r.write("c.txt", "c\n");
    r.ok(&["two"]);
    // Tags, a new branch, and a reset that does not move the branch are fine.
    assert_eq!(r.git_try(&["tag", "v1"]).code, 0);
    assert_eq!(r.git_try(&["branch", "other"]).code, 0);
    assert_eq!(r.git_try(&["reset", "-q"]).code, 0);
    // The commit that `git stone commit` makes is its own, so it passes.
    let o = r.stone(&["1", "commit", "-m", "stone one"]);
    assert_eq!(o.code, 0, "{}{}", o.stdout, o.stderr);
    let o = r.stone(&["commit-all"]);
    assert_eq!(o.code, 0, "{}{}", o.stdout, o.stderr);
    assert!(o.stdout.contains("No stones are left."), "{}", o.stdout);
}

#[test]
fn block_does_nothing_on_a_branch_without_stones() {
    let r = Repo::new();
    r.ok(&["guard", "block"]);
    r.write("x.txt", "x\n");
    r.git(&["add", "x.txt"]);
    assert_eq!(r.git_try(&["commit", "-q", "-m", "plain"]).code, 0);
    // After the last stone is gone the guard stops caring.
    r.write("y.txt", "y\n");
    r.ok(&["one"]);
    r.ok(&["drop", "all"]);
    r.git(&["add", "y.txt"]);
    assert_eq!(r.git_try(&["commit", "-q", "-m", "plain again"]).code, 0);
}

#[test]
fn stones_of_another_branch_do_not_block_this_one() {
    let r = with_a_stone_and_a_staged_file();
    r.ok(&["guard", "block"]);
    r.git(&["stash", "-q"]);
    r.git(&["switch", "-q", "-c", "other"]);
    r.write("o.txt", "o\n");
    r.git(&["add", "o.txt"]);
    assert_eq!(r.git_try(&["commit", "-q", "-m", "on other"]).code, 0);
}

#[test]
fn warn_allows_the_command_but_says_so() {
    let r = with_a_stone_and_a_staged_file();
    r.ok(&["guard", "warn"]);
    let o = r.git_try(&["commit", "-q", "-m", "plain"]);
    assert_eq!(o.code, 0, "{}", o.stderr);
    assert!(
        o.stderr.contains("Warning: branch `main` has 1 stone"),
        "{}",
        o.stderr
    );
}

#[test]
fn off_removes_the_setting_and_the_hook() {
    let r = with_a_stone_and_a_staged_file();
    r.ok(&["guard", "block"]);
    assert!(r.ok(&["guard"]).contains("Guard: block"));
    let out = r.ok(&["guard", "off"]);
    assert!(
        out.contains("Guard is off") && out.contains("Hook removed"),
        "{out}"
    );
    assert!(!hook(&r).exists());
    assert!(r.ok(&["guard"]).contains("Guard: off"));
    assert_eq!(r.git_try(&["commit", "-q", "-m", "plain"]).code, 0);
}

#[test]
fn a_hook_of_your_own_is_never_touched() {
    let r = with_a_stone_and_a_staged_file();
    r.hook("reference-transaction", "echo mine");
    let mine = std::fs::read_to_string(hook(&r)).unwrap();
    let out = r.ok(&["guard", "block"]);
    assert!(
        out.contains("NOT active yet") && out.contains("git-stone guard-check"),
        "{out}"
    );
    assert_eq!(std::fs::read_to_string(hook(&r)).unwrap(), mine);
    // `off` leaves it too.
    r.ok(&["guard", "off"]);
    assert_eq!(std::fs::read_to_string(hook(&r)).unwrap(), mine);
}

#[test]
fn a_custom_hooks_folder_is_never_written_to() {
    let r = with_a_stone_and_a_staged_file();
    r.git(&["config", "core.hooksPath", ".husky"]);
    let out = r.ok(&["guard", "block"]);
    assert!(
        out.contains("core.hooksPath points elsewhere") && out.contains("NOT active yet"),
        "{out}"
    );
    assert!(!r.dir.join(".husky").exists());
    assert!(!hook(&r).exists());
}

#[test]
fn guard_check_never_blocks_by_mistake() {
    let r = Repo::new();
    // Outside a repository, with nonsense input, without a setting: always allow.
    let elsewhere = tempfile::tempdir().unwrap();
    for (dir, phase) in [
        (elsewhere.path(), "prepared"),
        (r.dir.as_path(), "prepared"),
        (r.dir.as_path(), "committed"),
    ] {
        let o = r.stone_in(dir, &["guard-check", phase]);
        assert_eq!(o.code, 0, "{}", o.stderr);
    }
    // Bad arguments are the only refusal, and they are not the block answer (3).
    assert_eq!(r.stone(&["guard-check"]).code, 1);
    assert_eq!(r.stone(&["guard", "maybe"]).code, 1);
}

#[test]
fn an_old_binary_that_does_not_know_guard_check_cannot_block() {
    // An old git-stone reads `guard-check` as a bad stone name and exits 1. Only exit code 3 means "block".
    let r = with_a_stone_and_a_staged_file();
    r.ok(&["guard", "block"]);
    let fake = tempfile::tempdir().unwrap();
    let old = fake.path().join("git-stone");
    std::fs::write(&old, "#!/bin/sh\nexit 1\n").unwrap();
    std::fs::set_permissions(&old, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    let mut path = std::ffi::OsString::from(fake.path());
    path.push(":");
    path.push(std::env::var_os("PATH").unwrap());
    let o = std::process::Command::new("git")
        .args(["commit", "-q", "-m", "plain"])
        .current_dir(&r.dir)
        .env("PATH", path)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    // And a missing binary allows too.
    assert!(std::fs::read_to_string(hook(&r))
        .unwrap()
        .contains("command -v git-stone"));
}
