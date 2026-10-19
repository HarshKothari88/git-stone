mod common;

use common::Repo;

/// Every path in a stone's snapshot.
fn paths(r: &Repo, branch: &str, id: u64) -> Vec<String> {
    let tree = r.stone_tree(branch, id);
    r.git(&["ls-tree", "-r", "--name-only", &tree])
        .lines()
        .map(String::from)
        .collect()
}

#[test]
fn excluded_untracked_files_stay_out_and_the_rule_is_not_kept() {
    let r = Repo::new();
    r.write("keep.txt", "keep\n");
    r.write(".env.local", "secret\n");
    let before = r.fingerprint();
    let out = r.ok(&["auth", "-x", ".env.local"]);
    assert!(out.contains("Left out: .env.local (1 file)"), "{out}");
    assert!(!out.contains("Remembered"), "{out}");
    r.assert_untouched(&before);
    assert_eq!(paths(&r, "main", 1), ["base.txt", "keep.txt"]);
    assert!(r.ok(&["exclude"]).contains("No exclude rules"));

    // Not remembered: the next stone takes the file.
    r.write("more.txt", "more\n");
    r.ok(&["next"]);
    assert!(paths(&r, "main", 2).contains(&".env.local".to_string()));
    assert!(r.temp_indexes().is_empty());
}

#[test]
fn a_modified_tracked_file_counts_as_unchanged() {
    let r = Repo::new();
    r.write("base.txt", "edited\n");
    r.write("new.txt", "new\n");
    r.ok(&["one", "-x", "base.txt"]);
    let diff = r.stone(&["diff", "1"]).stdout;
    assert!(
        diff.contains("new.txt") && !diff.contains("base.txt"),
        "{diff}"
    );
    // The file on disk is still the edited one.
    assert_eq!(r.read("base.txt"), "edited\n");

    // A deleted tracked file is kept too, not recorded as deleted.
    std::fs::remove_file(r.dir.join("base.txt")).unwrap();
    r.write("other.txt", "o\n");
    r.ok(&["two", "-x", "base.txt"]);
    assert!(paths(&r, "main", 2).contains(&"base.txt".to_string()));
}

#[test]
fn globs_match_at_every_depth() {
    let r = Repo::new();
    r.write("a.log", "1\n");
    r.write("src/deep/b.log", "1\n");
    r.write("src/c.ts", "1\n");
    r.ok(&["one", "-x", "*.log"]);
    assert_eq!(paths(&r, "main", 1), ["base.txt", "src/c.ts"]);
}

#[test]
fn remember_applies_to_later_stones_on_every_branch() {
    let r = Repo::new();
    r.write("a.log", "1\n");
    r.write("a.txt", "1\n");
    let out = r.ok(&["one", "-x", "*.log", "--remember"]);
    assert!(out.contains("Remembered for this repository"), "{out}");
    assert!(r.ok(&["exclude"]).contains("  *.log"));

    r.write("b.log", "2\n");
    r.write("b.txt", "2\n");
    let out = r.ok(&["two"]);
    assert!(out.contains("Left out: *.log (2 files)"), "{out}");
    assert_eq!(paths(&r, "main", 2), ["a.txt", "b.txt", "base.txt"]);

    // Another branch, same rule.
    r.git(&["switch", "-q", "-c", "other"]);
    r.write("c.log", "3\n");
    r.write("c.txt", "3\n");
    r.ok(&["three"]);
    assert!(!paths(&r, "other", 1).iter().any(|p| p.ends_with(".log")));
    assert!(r.ok(&["status"]).contains("Excluded by rule: *.log"));
}

#[test]
fn remove_and_clear_include_the_files_again() {
    let r = Repo::new();
    r.write("a.log", "1\n");
    r.write("a.txt", "1\n");
    r.ok(&["one", "-x", "*.log", "-x", "tmp/", "--remember"]);

    let out = r.ok(&["exclude", "--remove", "*.log"]);
    assert!(out.contains("Removed"), "{out}");
    let list = r.ok(&["exclude"]);
    assert!(list.contains("tmp/") && !list.contains("*.log"), "{list}");

    r.write("b.txt", "2\n");
    r.ok(&["two"]);
    // The stone after the rule is removed holds every change made while it was excluded.
    assert!(paths(&r, "main", 2).contains(&"a.log".to_string()));

    let o = r.stone(&["exclude", "--remove", "nope"]);
    assert_eq!(o.code, 1);
    r.ok(&["exclude", "--clear"]);
    assert!(r.ok(&["exclude"]).contains("No exclude rules"));
}

#[test]
fn rules_typed_in_a_subfolder_are_saved_from_the_top() {
    let r = Repo::new();
    r.write("src/gen/x.ts", "1\n");
    r.write("src/y.ts", "1\n");
    let o = r.stone_in(&r.dir.join("src"), &["one", "-x", "gen/", "--remember"]);
    assert_eq!(o.code, 0, "{}{}", o.stdout, o.stderr);
    assert!(r.ok(&["exclude"]).contains("  src/gen/"));
    assert_eq!(paths(&r, "main", 1), ["base.txt", "src/y.ts"]);

    // A leading slash means the top of the repository.
    r.write("z.txt", "z\n");
    r.write("w.txt", "w\n");
    let o = r.stone_in(&r.dir.join("src"), &["two", "-x", "/z.txt"]);
    assert_eq!(o.code, 0, "{}{}", o.stdout, o.stderr);
    assert_eq!(paths(&r, "main", 2), ["base.txt", "src/y.ts", "w.txt"]);
}

#[test]
fn committing_leaves_the_excluded_file_and_its_staged_copy_alone() {
    let r = Repo::new();
    r.write("base.txt", "staged edit\n");
    r.git(&["add", "base.txt"]);
    r.write("feature.txt", "f\n");
    r.ok(&["feature", "-x", "base.txt"]);
    r.ok(&["1", "commit", "-m", "feature"]);

    let committed = r.git(&["show", "--name-only", "--format=", "HEAD"]);
    assert_eq!(committed.trim(), "feature.txt");
    assert_eq!(r.read("base.txt"), "staged edit\n");
    assert_eq!(
        r.git(&["diff", "--cached", "--name-only"]).trim(),
        "base.txt"
    );
}

#[test]
fn nothing_changed_mentions_the_rules() {
    let r = Repo::new();
    r.write("a.log", "1\n");
    let o = r.stone(&["one", "-x", "*.log"]);
    assert_eq!(o.code, 1);
    assert!(o.stderr.contains("Nothing changed"), "{}", o.stderr);
    assert!(o.stderr.contains("exclude rules"), "{}", o.stderr);
    assert!(!r.state_ref_exists("refs/stones/heads/main"));
    // The rule was not kept, because no stone was made.
    let o = r.stone(&["one", "-x", "*.log", "--remember"]);
    assert_eq!(o.code, 1);
    assert!(r.ok(&["exclude"]).contains("No exclude rules"));
}

#[test]
fn bad_input_is_refused() {
    let r = Repo::new();
    r.write("a.txt", "1\n");
    for args in [
        &["one", "-x", ""][..],
        &["one", "-x", "../outside"],
        &["one", "--remember"],
        &["one", "-x"],
    ] {
        let o = r.stone(args);
        assert_eq!(o.code, 1, "{args:?}: {}{}", o.stdout, o.stderr);
    }
    assert!(!r.state_ref_exists("refs/stones/heads/main"));
}

#[test]
fn exclude_works_on_an_unborn_branch_and_with_review_and_diff() {
    let r = Repo::unborn();
    r.write("a.txt", "1\n");
    r.write("b.log", "1\n");
    r.ok(&["one", "-x", "*.log", "--remember"]);
    assert_eq!(paths(&r, "main", 1), ["a.txt"]);

    r.write("c.log", "2\n");
    r.write("d.txt", "2\n");
    let review = r.ok(&["review"]);
    assert!(
        review.contains("d.txt") && !review.contains("c.log"),
        "{review}"
    );
    let diff = r.stone(&["diff", "current"]).stdout;
    assert!(diff.contains("d.txt") && !diff.contains("c.log"), "{diff}");
}
