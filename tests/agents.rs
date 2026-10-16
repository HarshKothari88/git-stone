mod common;

use common::Repo;

#[test]
fn agents_prints_the_memory_block_and_needs_no_repository() {
    let dir = tempfile::tempdir().unwrap();
    let r = Repo::new();
    let o = r.stone_in(dir.path(), &["agents"]);
    assert_eq!(o.code, 0, "{}", o.stderr);
    assert!(
        o.stdout.starts_with("## Git Stone (task boundaries)"),
        "{}",
        o.stdout
    );
    assert!(o.stdout.contains("Always add `-d`"));
    assert_eq!(r.ok(&["agents"]), o.stdout);
}
