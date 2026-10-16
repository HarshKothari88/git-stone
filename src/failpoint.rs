//! Test hook: `GIT_STONE_FAILPOINT=<name>` makes the process exit with code 99 at that point.

pub fn failpoint(name: &str) {
    if std::env::var("GIT_STONE_FAILPOINT").is_ok_and(|v| v == name) {
        eprintln!("git-stone: failpoint `{name}` hit");
        std::process::exit(99);
    }
}
