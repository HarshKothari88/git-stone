//! The stone state of one branch (or one detached HEAD): what it looks like on disk, how to read it, how to save it.
//!
//! All stones of a scope live in ONE state commit behind ONE ref, so a single compare-and-swap
//! `update-ref` changes the whole chain atomically (A1). The commit's tree holds:
//!   meta        text blob with the chain order, names and ids
//!   trees/<id>  one entry per stone, pointing at that stone's snapshot tree

use std::fmt;

use crate::git::{Git, GitError};
use crate::repo::{Repo, Scope};

const HEADER: &str = "git-stone 1";
const MAX_NAME_BYTES: usize = 200;
const MAX_DESCRIPTION_BYTES: usize = 10_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stone {
    pub id: u64,
    /// Unix seconds.
    pub created: u64,
    pub name: String,
    /// What the stone is for, in detail. Stored apart from `meta` (it can be long and have newlines).
    pub description: Option<String>,
    /// The snapshot: a tree of the whole repository.
    pub tree: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct State {
    /// The full branch ref, or the word `detached`.
    pub branch: String,
    /// The commit the stones are based on. `None` on an unborn branch.
    pub anchor: Option<String>,
    /// The id the next stone gets. Never goes down, even when the chain is empty (I7).
    pub next_id: u64,
    /// In chain order.
    pub stones: Vec<Stone>,
}

#[derive(Debug)]
pub enum StateError {
    /// What is stored cannot be trusted. We never guess.
    Corrupt(String),
    /// Another git-stone process changed the ref between our read and our write.
    Conflict,
    Git(GitError),
}

impl StateError {
    /// 1 = run it again, 2 = internal or git failure.
    pub fn exit_code(&self) -> i32 {
        match self {
            StateError::Conflict => 1,
            StateError::Corrupt(_) | StateError::Git(_) => 2,
        }
    }
}

impl fmt::Display for StateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StateError::Corrupt(why) => write!(f, "stone state is corrupt: {why}"),
            StateError::Conflict => write!(
                f,
                "Another git stone command changed the stones at the same time. Nothing was saved. Run it again."
            ),
            StateError::Git(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for StateError {}

impl From<GitError> for StateError {
    fn from(e: GitError) -> StateError {
        StateError::Git(e)
    }
}

fn corrupt<T>(why: impl Into<String>) -> Result<T, StateError> {
    Err(StateError::Corrupt(why.into()))
}

/// Check a stone name and return it trimmed (A3).
pub fn validate_name(name: &str) -> Result<String, String> {
    let name = name.trim_matches(' ');
    if name.is_empty() {
        return Err("A stone name cannot be empty.".into());
    }
    if name.len() > MAX_NAME_BYTES {
        return Err(format!(
            "A stone name can be at most {MAX_NAME_BYTES} bytes; this one is {}.",
            name.len()
        ));
    }
    if name.chars().any(char::is_control) {
        return Err("A stone name cannot contain control characters (newline, tab, ...).".into());
    }
    Ok(name.to_string())
}

/// Check a description and return it trimmed. `None` means "no description" (empty after trimming).
pub fn validate_description(text: &str) -> Result<Option<String>, String> {
    let text = text.trim();
    if text.len() > MAX_DESCRIPTION_BYTES {
        return Err(format!(
            "A stone description can be at most {MAX_DESCRIPTION_BYTES} bytes; this one is {}.",
            text.len()
        ));
    }
    if text.contains('\0') {
        return Err("A stone description cannot contain a NUL byte.".into());
    }
    Ok((!text.is_empty()).then(|| text.to_string()))
}

/// The `branch` line of the meta file for a scope.
pub fn branch_field(scope: &Scope) -> &str {
    match scope {
        Scope::Branch(full) => full,
        Scope::Detached => "detached",
    }
}

/// A decimal number written the one canonical way: no sign, no leading zeros.
fn canonical_u64(s: &str) -> Option<u64> {
    let canonical =
        !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()) && (s == "0" || !s.starts_with('0'));
    if canonical {
        s.parse().ok()
    } else {
        None
    }
}

fn is_oid(s: &str, hex_len: usize) -> bool {
    s.len() == hex_len && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

fn oid_of(out: Vec<u8>) -> String {
    String::from_utf8_lossy(&out).trim().to_string()
}

/// `None` when the ref does not exist.
fn read_ref(git: &Git, name: &str) -> Result<Option<String>, StateError> {
    match git.run(["rev-parse", "-q", "--verify", name]) {
        Ok(out) => Ok(Some(oid_of(out))),
        Err(e) if e.code == Some(1) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

impl State {
    pub fn new(branch: String, anchor: Option<String>) -> State {
        State {
            branch,
            anchor,
            next_id: 1,
            stones: Vec::new(),
        }
    }

    /// The meta text (A3):
    ///
    /// ```text
    /// git-stone 1
    /// branch refs/heads/feature/auth
    /// anchor <full oid>
    /// next-id 4
    /// stone 1 1760600000 auth
    /// ```
    pub fn serialize(&self) -> String {
        let mut out = format!(
            "{HEADER}\nbranch {}\nanchor {}\nnext-id {}\n",
            self.branch,
            self.anchor.as_deref().unwrap_or("none"),
            self.next_id
        );
        for s in &self.stones {
            out.push_str(&format!("stone {} {} {}\n", s.id, s.created, s.name));
        }
        out
    }

    /// Read the meta text and check it against the `trees/` and `desc/` entries and the place it was read from.
    /// `entries` are `(id, tree oid)` and `descriptions` are `(id, text)`, both in any order.
    /// `hex_len` is the length of an object id in this repo.
    pub fn parse(
        meta: &str,
        entries: &[(u64, String)],
        descriptions: &[(u64, String)],
        expected_branch: &str,
        hex_len: usize,
    ) -> Result<State, StateError> {
        let Some(body) = meta.strip_suffix('\n') else {
            return corrupt("meta does not end with a newline");
        };
        let mut lines = body.split('\n');
        if lines.next() != Some(HEADER) {
            return corrupt("meta has an unknown header");
        }
        let branch = field(lines.next(), "branch")?;
        if branch != expected_branch {
            return corrupt(format!(
                "meta belongs to `{branch}`, but it was read for `{expected_branch}`"
            ));
        }
        let anchor = match field(lines.next(), "anchor")? {
            "none" => None,
            oid if is_oid(oid, hex_len) => Some(oid.to_string()),
            other => return corrupt(format!("anchor `{other}` is not an object id")),
        };
        let next_id = match canonical_u64(field(lines.next(), "next-id")?) {
            Some(n) if n >= 1 => n,
            _ => return corrupt("next-id is not a positive number"),
        };

        let mut stones: Vec<Stone> = Vec::new();
        for line in lines {
            let Some(rest) = line.strip_prefix("stone ") else {
                return corrupt(format!("unexpected line `{line}`"));
            };
            let mut parts = rest.splitn(3, ' ');
            let (Some(id), Some(created), Some(name)) = (parts.next(), parts.next(), parts.next())
            else {
                return corrupt(format!("malformed stone line `{line}`"));
            };
            let (Some(id), Some(created)) = (canonical_u64(id), canonical_u64(created)) else {
                return corrupt(format!("malformed stone line `{line}`"));
            };
            if id == 0 || id >= next_id {
                return corrupt(format!("stone id {id} is not below next-id {next_id}"));
            }
            if stones.last().is_some_and(|prev| prev.id >= id) {
                return corrupt(format!(
                    "stone id {id} is not larger than the one before it"
                ));
            }
            if validate_name(name).as_deref() != Ok(name) {
                return corrupt(format!("stone {id} has an invalid name"));
            }
            let Some((_, tree)) = entries.iter().find(|(entry_id, _)| *entry_id == id) else {
                return corrupt(format!("stone {id} has no snapshot in trees/"));
            };
            let description = match descriptions.iter().find(|(d, _)| *d == id) {
                // Stored text must already be in its trimmed, valid form.
                Some((_, text)) => match validate_description(text) {
                    Ok(Some(valid)) if valid == *text => Some(valid),
                    _ => return corrupt(format!("stone {id} has an invalid description")),
                },
                None => None,
            };
            stones.push(Stone {
                id,
                created,
                name: name.to_string(),
                description,
                tree: tree.clone(),
            });
        }
        // Every lookup above succeeded and ids are unique, so a longer list means extras.
        if entries.len() != stones.len() {
            return corrupt("trees/ holds snapshots that meta does not list");
        }
        let described = stones.iter().filter(|s| s.description.is_some()).count();
        if descriptions.len() != described {
            return corrupt("desc/ holds descriptions for stones that meta does not list");
        }
        Ok(State {
            branch: branch.to_string(),
            anchor,
            next_id,
            stones,
        })
    }

    /// Read the state of the current scope. Returns the state commit's oid too: it is the `old` value for `save`.
    pub fn load(repo: &Repo) -> Result<Option<(String, State)>, StateError> {
        State::load_ref(repo, &repo.scope.state_ref(), branch_field(&repo.scope))
    }

    /// Read the state behind any state ref. `expected_branch` is the `branch` line it must carry.
    pub fn load_ref(
        repo: &Repo,
        state_ref: &str,
        expected_branch: &str,
    ) -> Result<Option<(String, State)>, StateError> {
        let git = repo.git();
        let Some(oid) = read_ref(&git, state_ref)? else {
            return Ok(None);
        };
        // A git failure here means the objects are missing or are not what they should be.
        let unreadable = |e: GitError| match e.code {
            Some(_) => StateError::Corrupt(format!("cannot read the state commit: {e}")),
            None => StateError::Git(e),
        };
        let root = parse_root(&git.run(["ls-tree", "-z", &oid]).map_err(unreadable)?)?;
        let meta = git
            .run(["cat-file", "blob", &root.meta])
            .map_err(unreadable)?;
        let Ok(meta) = String::from_utf8(meta) else {
            return corrupt("meta is not valid UTF-8");
        };
        let listing = git
            .run(["ls-tree", "-z", &root.trees])
            .map_err(unreadable)?;
        let entries = parse_entries(&listing, "040000", "tree")?;

        // All descriptions in one `cat-file --batch` call, so reading a long chain stays cheap.
        let mut descriptions = Vec::new();
        if let Some(desc) = &root.desc {
            let listing = git.run(["ls-tree", "-z", desc]).map_err(unreadable)?;
            let blobs = parse_entries(&listing, "100644", "blob")?;
            let wanted: String = blobs.iter().map(|(_, oid)| format!("{oid}\n")).collect();
            let out = git
                .run_stdin(["cat-file", "--batch"], wanted.as_bytes())
                .map_err(unreadable)?;
            for ((id, _), text) in blobs.iter().zip(split_batch(&out, blobs.len())?) {
                let Ok(text) = String::from_utf8(text) else {
                    return corrupt(format!("the description of stone {id} is not valid UTF-8"));
                };
                descriptions.push((*id, text));
            }
        }

        let state = State::parse(
            &meta,
            &entries,
            &descriptions,
            expected_branch,
            repo.empty_tree.len(),
        )?;
        Ok(Some((oid, state)))
    }

    /// Write this state as a new state commit and move the ref to it, but only if the ref still
    /// holds `old` (`None` = must not exist yet). Returns the new commit's oid.
    /// A lost race is `StateError::Conflict`; nothing is changed then, and the objects we wrote are garbage for gc.
    pub fn save(&self, repo: &Repo, old: Option<&str>, op: &str) -> Result<String, StateError> {
        let git = repo.git();
        let meta = oid_of(git.run_stdin(
            ["hash-object", "-w", "--stdin"],
            self.serialize().as_bytes(),
        )?);

        let mut listing = Vec::new();
        for s in &self.stones {
            listing.extend(format!("040000 tree {}\t{}\0", s.tree, s.id).bytes());
        }
        // An empty listing makes the empty tree, which is the right `trees/` for an empty chain.
        let trees = oid_of(git.run_stdin(["mktree", "-z"], &listing)?);
        let mut root = format!("100644 blob {meta}\tmeta\0040000 tree {trees}\ttrees\0");

        // `desc/` exists only when some stone has a description.
        let mut descriptions = Vec::new();
        for s in &self.stones {
            if let Some(text) = &s.description {
                let blob =
                    oid_of(git.run_stdin(["hash-object", "-w", "--stdin"], text.as_bytes())?);
                descriptions.extend(format!("100644 blob {blob}\t{}\0", s.id).bytes());
            }
        }
        if !descriptions.is_empty() {
            let desc = oid_of(git.run_stdin(["mktree", "-z"], &descriptions)?);
            root.push_str(&format!("040000 tree {desc}\tdesc\0"));
        }
        let root = oid_of(git.run_stdin(["mktree", "-z"], root.as_bytes())?);

        // The anchor is the only parent: it keeps the base commit reachable, and no link to the
        // previous state commit lets dropped stones be garbage collected (A3).
        // A fixed identity, so saving works even when the user has not set one.
        let message = format!("git-stone: {op}");
        let mut args = vec!["commit-tree", &root];
        if let Some(anchor) = &self.anchor {
            args.extend(["-p", anchor]);
        }
        args.extend(["-m", &message]);
        let commit = oid_of(
            git.clone()
                .with_env("GIT_AUTHOR_NAME", "git-stone")
                .with_env("GIT_AUTHOR_EMAIL", "git-stone@localhost")
                .with_env("GIT_COMMITTER_NAME", "git-stone")
                .with_env("GIT_COMMITTER_EMAIL", "git-stone@localhost")
                .run(args)?,
        );

        // The one write that matters. `--create-reflog`: git keeps no reflog for custom namespaces by default.
        let state_ref = repo.scope.state_ref();
        let zero = repo.zero_oid();
        let swap = git.run([
            "update-ref",
            "--create-reflog",
            "-m",
            &message,
            &state_ref,
            &commit,
            old.unwrap_or(&zero),
        ]);
        if let Err(e) = swap {
            // Decide by looking at the ref, not at git's message (which depends on the language).
            return if read_ref(&git, &state_ref)?.as_deref() != old {
                Err(StateError::Conflict)
            } else {
                Err(e.into())
            };
        }
        Ok(commit)
    }
}

/// `<key> <value>` -> value
fn field<'a>(line: Option<&'a str>, key: &str) -> Result<&'a str, StateError> {
    match line.and_then(|l| l.strip_prefix(key)?.strip_prefix(' ')) {
        Some(value) => Ok(value),
        None => corrupt(format!("meta has no `{key}` line where it should")),
    }
}

/// The three things the state commit's root tree may hold.
struct Root {
    meta: String,
    trees: String,
    desc: Option<String>,
}

/// One record of `git ls-tree -z`: `<mode> <type> <oid>\t<name>\0`, as (mode, type, oid, name).
fn split_records(listing: &[u8]) -> Result<Vec<(&str, &str, &str, &str)>, StateError> {
    let mut out = Vec::new();
    for record in listing.split(|b| *b == 0).filter(|r| !r.is_empty()) {
        let Ok(text) = std::str::from_utf8(record) else {
            return corrupt("a tree entry is not valid UTF-8");
        };
        let Some((info, name)) = text.split_once('\t') else {
            return corrupt("a tree entry is malformed");
        };
        let info: Vec<&str> = info.split(' ').collect();
        let &[mode, kind, oid] = info.as_slice() else {
            return corrupt("a tree entry is malformed");
        };
        out.push((mode, kind, oid, name));
    }
    Ok(out)
}

fn parse_root(listing: &[u8]) -> Result<Root, StateError> {
    let (mut meta, mut trees, mut desc) = (None, None, None);
    for (mode, kind, oid, name) in split_records(listing)? {
        let slot = match (name, mode, kind) {
            ("meta", "100644", "blob") => &mut meta,
            ("trees", "040000", "tree") => &mut trees,
            ("desc", "040000", "tree") => &mut desc,
            _ => return corrupt(format!("the state commit has an unexpected entry `{name}`")),
        };
        *slot = Some(oid.to_string());
    }
    let (Some(meta), Some(trees)) = (meta, trees) else {
        return corrupt("the state commit has no meta or no trees/");
    };
    Ok(Root { meta, trees, desc })
}

/// Read the `ls-tree -z` listing of `trees/` or `desc/`: one entry per stone, named by its id.
/// Returns `(id, oid)`. `mode` and `kind` say what every entry must be.
fn parse_entries(listing: &[u8], mode: &str, kind: &str) -> Result<Vec<(u64, String)>, StateError> {
    let mut entries = Vec::new();
    for (m, k, oid, name) in split_records(listing)? {
        if m != mode || k != kind {
            return corrupt(format!("entry {name} is not a {kind}"));
        }
        let Some(id) = canonical_u64(name) else {
            return corrupt(format!("entry {name} is not a stone id"));
        };
        entries.push((id, oid.to_string()));
    }
    Ok(entries)
}

/// Cut `git cat-file --batch` output (`<oid> blob <size>\n<bytes>\n` per object) into the object bytes.
fn split_batch(mut out: &[u8], count: usize) -> Result<Vec<Vec<u8>>, StateError> {
    let mut blobs = Vec::with_capacity(count);
    for _ in 0..count {
        let Some(end) = out.iter().position(|b| *b == b'\n') else {
            return corrupt("a description could not be read");
        };
        let header = String::from_utf8_lossy(&out[..end]);
        let parts: Vec<&str> = header.split(' ').collect();
        let &[_, "blob", size] = parts.as_slice() else {
            return corrupt(format!("a description could not be read ({header})"));
        };
        let Ok(size) = size.parse::<usize>() else {
            return corrupt("a description could not be read");
        };
        let rest = &out[end + 1..];
        if rest.len() < size + 1 {
            return corrupt("a description could not be read");
        }
        blobs.push(rest[..size].to_vec());
        out = &rest[size + 1..];
    }
    Ok(blobs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repo::testutil::{commit_file, fixture_git, init};

    const OID_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const OID_T1: &str = "1111111111111111111111111111111111111111";
    const OID_T3: &str = "3333333333333333333333333333333333333333";
    const BRANCH: &str = "refs/heads/feature/auth";

    fn sample() -> State {
        State {
            branch: BRANCH.into(),
            anchor: Some(OID_A.into()),
            next_id: 4,
            stones: vec![
                Stone {
                    id: 1,
                    created: 1760600000,
                    name: "auth".into(),
                    description: None,
                    tree: OID_T1.into(),
                },
                Stone {
                    id: 3,
                    created: 1760600100,
                    name: "rate limit".into(),
                    description: None,
                    tree: OID_T3.into(),
                },
            ],
        }
    }

    fn sample_entries() -> Vec<(u64, String)> {
        vec![(3, OID_T3.into()), (1, OID_T1.into())]
    }

    fn parse(meta: &str) -> Result<State, StateError> {
        State::parse(meta, &sample_entries(), &[], BRANCH, 40)
    }

    fn assert_corrupt(result: Result<State, StateError>, needle: &str) {
        match result {
            Err(StateError::Corrupt(why)) => {
                assert!(why.contains(needle), "`{why}` does not mention `{needle}`")
            }
            other => panic!("expected corrupt state ({needle}), got {other:?}"),
        }
    }

    #[test]
    fn meta_text_is_exact() {
        let expected = format!(
            "git-stone 1\n\
             branch {BRANCH}\n\
             anchor {OID_A}\n\
             next-id 4\n\
             stone 1 1760600000 auth\n\
             stone 3 1760600100 rate limit\n"
        );
        assert_eq!(sample().serialize(), expected);
    }

    #[test]
    fn parse_round_trips_and_ignores_entry_order() {
        let state = sample();
        assert_eq!(parse(&state.serialize()).unwrap(), state);
    }

    #[test]
    fn unborn_anchor_and_empty_chain_round_trip() {
        let state = State {
            branch: BRANCH.into(),
            anchor: None,
            next_id: 7,
            stones: vec![],
        };
        let meta = state.serialize();
        assert!(meta.contains("anchor none\n"));
        assert_eq!(State::parse(&meta, &[], &[], BRANCH, 40).unwrap(), state);
    }

    #[test]
    fn hostile_names_survive_a_round_trip() {
        for name in [
            "$(rm -rf /)",
            "a;b",
            "-rf",
            "émoji 🪨",
            "  inner  spaces  ",
            "42",
            "tab\u{a0}nbsp",
        ] {
            let mut state = sample();
            state.stones[0].name = validate_name(name).unwrap();
            assert_eq!(parse(&state.serialize()).unwrap(), state, "{name}");
        }
    }

    #[test]
    fn corrupt_meta_is_refused() {
        let good = sample().serialize();
        let cases = [
            (good.replace("git-stone 1", "git-stone 2"), "header"),
            (good.trim_end().to_string(), "newline"),
            (good.replace(BRANCH, "refs/heads/other"), "belongs to"),
            (good.replace("branch ", "bran "), "`branch` line"),
            (good.replace(OID_A, "xyz"), "not an object id"),
            (
                good.replace(OID_A, &OID_A.to_uppercase()),
                "not an object id",
            ),
            (good.replace("next-id 4", "next-id four"), "next-id"),
            (good.replace("next-id 4", "next-id 04"), "next-id"),
            (good.replace("next-id 4", "next-id 3"), "not below next-id"),
            (good.replace("stone 3 ", "stone 1 "), "not larger"),
            (good.replace("stone 1 ", "stone 0 "), "not below next-id"),
            (good.replace("stone 3 1760600100", "stone 3"), "malformed"),
            (good.replace("1760600000", "-5"), "malformed"),
            (good.replace("stone 3 ", "stone 03 "), "malformed"),
            (format!("{good}\n"), "unexpected line"),
            (format!("{good}junk\n"), "unexpected line"),
            (good.replace("rate limit", " rate limit"), "invalid name"),
            (good.replace("rate limit", "rate\tlimit"), "invalid name"),
            (good.replace("rate limit", ""), "invalid name"),
            (good.replace("stone 1 ", "stone 2 "), "no snapshot"),
        ];
        for (meta, needle) in cases {
            assert_corrupt(parse(&meta), needle);
        }
    }

    #[test]
    fn meta_and_trees_must_match_exactly() {
        let meta = sample().serialize();
        let missing = [(1, OID_T1.to_string())];
        assert_corrupt(
            State::parse(&meta, &missing, &[], BRANCH, 40),
            "no snapshot",
        );
        let mut extra = sample_entries();
        extra.push((2, OID_T1.into()));
        assert_corrupt(
            State::parse(&meta, &extra, &[], BRANCH, 40),
            "does not list",
        );
    }

    #[test]
    fn oid_length_follows_the_object_format() {
        let meta = sample().serialize();
        assert_corrupt(
            State::parse(&meta, &sample_entries(), &[], BRANCH, 64),
            "not an object id",
        );
    }

    #[test]
    fn tree_listing_is_checked() {
        let ok = format!("040000 tree {OID_T1}\t1\0040000 tree {OID_T3}\t10\0");
        assert_eq!(
            parse_entries(ok.as_bytes(), "040000", "tree").unwrap(),
            [(1, OID_T1.to_string()), (10, OID_T3.to_string())]
        );
        for bad in [
            format!("100644 blob {OID_T1}\t1\0"),
            format!("040000 tree {OID_T1}\tone\0"),
            format!("040000 tree {OID_T1}\t01\0"),
            format!("040000 tree {OID_T1}\0"),
        ] {
            assert!(
                matches!(
                    parse_entries(bad.as_bytes(), "040000", "tree"),
                    Err(StateError::Corrupt(_))
                ),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn name_rules() {
        assert_eq!(validate_name("  auth  ").unwrap(), "auth");
        assert!(validate_name("").is_err());
        assert!(validate_name("   ").is_err());
        assert!(validate_name("a\nb").is_err());
        assert!(validate_name("a\tb").is_err());
        assert!(validate_name("a\rb").is_err());
        assert!(validate_name("a\0b").is_err());
        assert!(validate_name("a\u{1b}[31m").is_err());
        assert!(validate_name(&"x".repeat(200)).is_ok());
        assert!(validate_name(&"x".repeat(201)).is_err());
        // 200 BYTES, not characters.
        assert!(validate_name(&"é".repeat(100)).is_ok());
        assert!(validate_name(&"é".repeat(101)).is_err());
    }

    fn desc_entries() -> Vec<(u64, String)> {
        vec![(3, "Adds rate limiting.\nCovers /login.".into())]
    }

    #[test]
    fn descriptions_attach_to_their_stone() {
        let state = State::parse(
            &sample().serialize(),
            &sample_entries(),
            &desc_entries(),
            BRANCH,
            40,
        )
        .unwrap();
        assert_eq!(state.stones[0].description, None);
        assert_eq!(
            state.stones[1].description.as_deref(),
            Some("Adds rate limiting.\nCovers /login.")
        );
    }

    #[test]
    fn bad_descriptions_are_corrupt() {
        let meta = sample().serialize();
        let run =
            |descs: Vec<(u64, String)>| State::parse(&meta, &sample_entries(), &descs, BRANCH, 40);
        assert_corrupt(run(vec![(2, "for nobody".into())]), "desc/");
        assert_corrupt(run(vec![(3, "a".into()), (3, "a".into())]), "desc/");
        assert_corrupt(run(vec![(3, " padded ".into())]), "invalid description");
        assert_corrupt(run(vec![(3, "".into())]), "invalid description");
        assert_corrupt(run(vec![(3, "nul\0byte".into())]), "invalid description");
        assert_corrupt(run(vec![(3, "x".repeat(10_001))]), "invalid description");
        assert!(run(vec![(3, "x".repeat(10_000))]).is_ok());
    }

    #[test]
    fn description_rules() {
        assert_eq!(
            validate_description("  why  ").unwrap().as_deref(),
            Some("why")
        );
        assert_eq!(
            validate_description("line 1\n\nline 3\n")
                .unwrap()
                .as_deref(),
            Some("line 1\n\nline 3")
        );
        assert_eq!(
            validate_description("tab\there").unwrap().as_deref(),
            Some("tab\there")
        );
        assert_eq!(validate_description("").unwrap(), None);
        assert_eq!(validate_description(" \n\t ").unwrap(), None);
        assert!(validate_description("a\0b").is_err());
        assert!(validate_description(&"x".repeat(10_000)).is_ok());
        assert!(validate_description(&"x".repeat(10_001)).is_err());
        // 10000 BYTES, not characters.
        assert!(validate_description(&"é".repeat(5_000)).is_ok());
        assert!(validate_description(&"é".repeat(5_001)).is_err());
    }

    #[test]
    fn batch_output_is_split_by_size() {
        let out = b"aaa blob 3\nabc\nbbb blob 4\na\nb\n\nccc blob 0\n\n";
        let blobs = split_batch(out, 3).unwrap();
        assert_eq!(blobs, [b"abc".to_vec(), b"a\nb\n".to_vec(), b"".to_vec()]);
        assert!(split_batch(b"aaa missing\n", 1).is_err());
        assert!(split_batch(b"aaa blob 9\nabc\n", 1).is_err());
        assert!(split_batch(b"", 1).is_err());
    }

    // ---- tests against a real repository ----

    struct Fixture {
        dir: tempfile::TempDir,
        git: Git,
    }

    impl Fixture {
        /// A repo with one commit on `main`.
        fn born() -> Fixture {
            let dir = tempfile::tempdir().unwrap();
            let git = init(dir.path());
            commit_file(&git, dir.path(), "a", "a");
            Fixture { dir, git }
        }

        fn repo(&self) -> Repo {
            Repo::discover_in(self.dir.path()).unwrap()
        }

        /// A tree object that nothing else in the repo points at.
        fn loose_tree(&self, text: &str) -> String {
            let blob = oid_of(
                self.git
                    .run_stdin(["hash-object", "-w", "--stdin"], text.as_bytes())
                    .unwrap(),
            );
            let entry = format!("100644 blob {blob}\tf\0");
            oid_of(
                self.git
                    .run_stdin(["mktree", "-z"], entry.as_bytes())
                    .unwrap(),
            )
        }

        fn state_for(&self, repo: &Repo, stones: &[(&str, &str)]) -> State {
            let mut state = State::new(
                match &repo.scope {
                    Scope::Branch(b) => b.clone(),
                    Scope::Detached => "detached".into(),
                },
                repo.head.clone(),
            );
            for (name, text) in stones {
                state.stones.push(Stone {
                    id: state.next_id,
                    created: 1760600000 + state.next_id,
                    name: (*name).into(),
                    description: None,
                    tree: self.loose_tree(text),
                });
                state.next_id += 1;
            }
            state
        }

        fn out(&self, args: &[&str]) -> String {
            String::from_utf8(self.git.run(args).unwrap()).unwrap()
        }
    }

    #[test]
    fn save_then_load_round_trips() {
        let fx = Fixture::born();
        let repo = fx.repo();
        assert!(State::load(&repo).unwrap().is_none());

        let state = fx.state_for(&repo, &[("auth", "one"), ("rate limit", "two")]);
        let oid = state.save(&repo, None, "create 2").unwrap();
        let (loaded_oid, loaded) = State::load(&repo).unwrap().unwrap();
        assert_eq!(loaded_oid, oid);
        assert_eq!(loaded, state);
    }

    #[test]
    fn state_commit_layout() {
        let fx = Fixture::born();
        let repo = fx.repo();
        let state = fx.state_for(&repo, &[("auth", "one"), ("rate limit", "two")]);
        state.save(&repo, None, "create 2").unwrap();
        let state_ref = repo.scope.state_ref();
        assert_eq!(state_ref, "refs/stones/heads/main");

        // The commit: parent is exactly the anchor, message says what happened.
        let commit = fx.out(&["cat-file", "-p", &state_ref]);
        println!("{commit}");
        assert!(commit.contains(&format!("parent {}\n", repo.head.as_ref().unwrap())));
        assert_eq!(
            commit.matches("\nparent ").count() + commit.starts_with("parent ") as usize,
            1
        );
        assert!(commit.contains("author git-stone <git-stone@localhost>"));
        assert!(commit.ends_with("\ngit-stone: create 2\n"));

        // The tree: meta + trees/<id>.
        let root = fx.out(&["ls-tree", &state_ref]);
        println!("{root}");
        assert!(root.contains("100644 blob") && root.contains("\tmeta\n"));
        assert!(root.contains("040000 tree") && root.contains("\ttrees\n"));
        let trees = fx.out(&["ls-tree", &format!("{state_ref}:trees")]);
        println!("{trees}");
        assert_eq!(trees.lines().count(), 2);
        assert_eq!(
            fx.out(&["cat-file", "-p", &format!("{state_ref}:meta")]),
            state.serialize()
        );
        // The snapshot is reachable by a plain path.
        let tree_1 = fx.out(&["rev-parse", &format!("{state_ref}:trees/1")]);
        assert_eq!(tree_1.trim(), state.stones[0].tree);
    }

    #[test]
    fn empty_chain_keeps_next_id() {
        let fx = Fixture::born();
        let repo = fx.repo();
        let mut state = fx.state_for(&repo, &[("auth", "one")]);
        let oid = state.save(&repo, None, "create 1").unwrap();
        state.stones.clear(); // next_id stays 2
        state.save(&repo, Some(&oid), "drop 1").unwrap();
        let (_, loaded) = State::load(&repo).unwrap().unwrap();
        assert!(loaded.stones.is_empty());
        assert_eq!(loaded.next_id, 2);
    }

    #[test]
    fn unborn_branch_state_has_no_parent() {
        let dir = tempfile::tempdir().unwrap();
        let git = init(dir.path());
        let fx = Fixture { dir, git };
        let repo = fx.repo();
        assert_eq!(repo.head, None);
        let state = fx.state_for(&repo, &[("first", "one")]);
        assert_eq!(state.anchor, None);
        state.save(&repo, None, "create 1").unwrap();
        let commit = fx.out(&["cat-file", "-p", &repo.scope.state_ref()]);
        assert!(!commit.contains("parent "));
        let (_, loaded) = State::load(&repo).unwrap().unwrap();
        assert_eq!(loaded, state);
    }

    #[test]
    fn detached_head_uses_its_own_ref() {
        let fx = Fixture::born();
        fx.git.run(["checkout", "-q", "--detach"]).unwrap();
        let repo = fx.repo();
        assert_eq!(repo.scope, Scope::Detached);
        let state = fx.state_for(&repo, &[("auth", "one")]);
        state.save(&repo, None, "create 1").unwrap();
        assert!(fx
            .out(&["for-each-ref", "refs/worktree/stones/"])
            .contains("refs/worktree/stones/detached"));
        assert_eq!(State::load(&repo).unwrap().unwrap().1, state);
        // The branch's chain is separate.
        fx.git.run(["checkout", "-q", "main"]).unwrap();
        assert!(State::load(&fx.repo()).unwrap().is_none());
    }

    #[test]
    fn second_save_with_the_same_old_value_conflicts() {
        let fx = Fixture::born();
        let repo = fx.repo();
        let first = fx.state_for(&repo, &[("auth", "one")]);
        let base = first.save(&repo, None, "create 1").unwrap();

        let mut a = first.clone();
        a.stones.clear();
        let mut b = first.clone();
        b.stones[0].name = "other".into();
        let winner = a.save(&repo, Some(&base), "drop 1").unwrap();
        let loser = b.save(&repo, Some(&base), "rename");
        assert!(matches!(loser, Err(StateError::Conflict)), "{loser:?}");
        assert_eq!(loser.unwrap_err().exit_code(), 1);
        // The ref still holds the winner, untouched.
        assert_eq!(State::load(&repo).unwrap().unwrap().0, winner);

        // "Must not exist yet" also loses when the ref exists.
        let create_again = first.save(&repo, None, "create 1");
        assert!(matches!(create_again, Err(StateError::Conflict)));
    }

    #[test]
    fn save_ignores_signing_config() {
        let fx = Fixture::born();
        let repo = fx.repo();
        fx.git.run(["config", "commit.gpgSign", "true"]).unwrap();
        fx.git.run(["config", "gpg.program", "false"]).unwrap();
        let state = fx.state_for(&repo, &[("auth", "one")]);
        state.save(&repo, None, "create 1").unwrap();
        assert!(fx
            .out(&["cat-file", "-p", &repo.scope.state_ref()])
            .find("gpgsig")
            .is_none());
    }

    #[test]
    fn reflog_records_every_save() {
        let fx = Fixture::born();
        let repo = fx.repo();
        let mut state = fx.state_for(&repo, &[("auth", "one")]);
        let first = state.save(&repo, None, "create 1").unwrap();
        state.stones.clear();
        state.save(&repo, Some(&first), "drop 1").unwrap();
        let log = fx.out(&["reflog", "show", "--format=%gs", &repo.scope.state_ref()]);
        println!("{log}");
        assert_eq!(log, "git-stone: drop 1\ngit-stone: create 1\n");
    }

    #[test]
    fn gc_keeps_live_stone_trees() {
        let fx = Fixture::born();
        let repo = fx.repo();
        let state = fx.state_for(&repo, &[("auth", "live one"), ("rate limit", "live two")]);
        let orphan = fx.loose_tree("nobody points here");
        state.save(&repo, None, "create 2").unwrap();
        // Drop the reflog too, so only real reachability counts.
        fx.git
            .run(["reflog", "expire", "--expire=now", "--all"])
            .unwrap();
        fx.git.run(["gc", "-q", "--prune=now"]).unwrap();
        for stone in &state.stones {
            fx.git.run(["cat-file", "-e", &stone.tree]).unwrap();
        }
        assert!(
            fx.git.run(["cat-file", "-e", &orphan]).is_err(),
            "gc should remove unreferenced objects"
        );
        fx.git.run(["fsck", "--no-dangling"]).unwrap();
        assert_eq!(State::load(&repo).unwrap().unwrap().1, state);
    }

    #[test]
    fn missing_pieces_are_reported_as_corrupt() {
        let fx = Fixture::born();
        let repo = fx.repo();
        // A ref that points at an ordinary commit has no `meta`.
        fx.git
            .run(["update-ref", &repo.scope.state_ref(), "HEAD"])
            .unwrap();
        let err = State::load(&repo).unwrap_err();
        assert!(matches!(err, StateError::Corrupt(_)), "{err:?}");
        assert_eq!(err.exit_code(), 2);
        assert!(err.to_string().starts_with("stone state is corrupt"));
    }

    #[test]
    fn state_read_for_another_branch_is_corrupt() {
        let fx = Fixture::born();
        let repo = fx.repo();
        let state = fx.state_for(&repo, &[("auth", "one")]);
        state.save(&repo, None, "create 1").unwrap();
        // Copy the state ref to another branch's name: the meta still says `main`.
        fx.git.run(["branch", "other"]).unwrap();
        fx.git
            .run([
                "update-ref",
                "refs/stones/heads/other",
                &repo.scope.state_ref(),
            ])
            .unwrap();
        fx.git.run(["checkout", "-q", "other"]).unwrap();
        let err = State::load(&fx.repo()).unwrap_err();
        assert!(err.to_string().contains("belongs to"), "{err}");
    }

    #[test]
    fn sha256_repo_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let git = fixture_git(dir.path());
        git.run(["init", "-q", "-b", "main", "--object-format=sha256"])
            .unwrap();
        commit_file(&git, dir.path(), "a", "a");
        let fx = Fixture { dir, git };
        let repo = fx.repo();
        assert_eq!(repo.object_format, "sha256");
        let state = fx.state_for(&repo, &[("auth", "one")]);
        assert_eq!(state.anchor.as_ref().unwrap().len(), 64);
        let oid = state.save(&repo, None, "create 1").unwrap();
        assert_eq!(oid.len(), 64);
        assert_eq!(State::load(&repo).unwrap().unwrap().1, state);
        // The "must not exist" check uses a 64-zero id here.
        assert_eq!(repo.zero_oid(), "0".repeat(64));
        assert!(matches!(
            state.save(&repo, None, "again"),
            Err(StateError::Conflict)
        ));
    }

    #[test]
    fn descriptions_round_trip_through_git() {
        let fx = Fixture::born();
        let repo = fx.repo();
        let mut state = fx.state_for(
            &repo,
            &[("auth", "one"), ("middle", "two"), ("rate limit", "three")],
        );
        state.stones[0].description = Some("Adds JWT login.\nCovers /login and /refresh.".into());
        state.stones[2].description = Some("émoji 🪨 and $(rm -rf /) and -rf".into());
        state.save(&repo, None, "create 3").unwrap();
        assert_eq!(State::load(&repo).unwrap().unwrap().1, state);

        // Layout: desc/<id> only for the stones that have one, raw text.
        let state_ref = repo.scope.state_ref();
        let listing = fx.out(&["ls-tree", &format!("{state_ref}:desc")]);
        println!("{listing}");
        assert_eq!(listing.lines().count(), 2);
        assert_eq!(
            fx.out(&["cat-file", "-p", &format!("{state_ref}:desc/1")]),
            "Adds JWT login.\nCovers /login and /refresh."
        );
        assert!(fx
            .git
            .run(["cat-file", "-e", &format!("{state_ref}:desc/2")])
            .is_err());
    }

    #[test]
    fn no_description_means_no_desc_directory() {
        let fx = Fixture::born();
        let repo = fx.repo();
        fx.state_for(&repo, &[("auth", "one")])
            .save(&repo, None, "create 1")
            .unwrap();
        let root = fx.out(&["ls-tree", "--name-only", &repo.scope.state_ref()]);
        assert_eq!(root, "meta\ntrees\n");
    }

    #[test]
    fn a_large_description_and_many_stones_load_in_one_pass() {
        let fx = Fixture::born();
        let repo = fx.repo();
        let mut state = fx.state_for(&repo, &[("a", "1"), ("b", "2"), ("c", "3")]);
        for (i, s) in state.stones.iter_mut().enumerate() {
            s.description = Some(format!("{i}\n{}", "x".repeat(10_000 - 2)));
        }
        state.save(&repo, None, "create 3").unwrap();
        assert_eq!(State::load(&repo).unwrap().unwrap().1, state);
    }

    #[test]
    fn gc_keeps_descriptions_of_live_stones() {
        let fx = Fixture::born();
        let repo = fx.repo();
        let mut state = fx.state_for(&repo, &[("auth", "one")]);
        state.stones[0].description = Some("keep me".into());
        state.save(&repo, None, "create 1").unwrap();
        fx.git
            .run(["reflog", "expire", "--expire=now", "--all"])
            .unwrap();
        fx.git.run(["gc", "-q", "--prune=now"]).unwrap();
        fx.git.run(["fsck", "--no-dangling"]).unwrap();
        assert_eq!(State::load(&repo).unwrap().unwrap().1, state);
    }

    #[test]
    fn stray_entries_in_the_state_commit_are_corrupt() {
        let fx = Fixture::born();
        let repo = fx.repo();
        let state = fx.state_for(&repo, &[("auth", "one")]);
        let oid = state.save(&repo, None, "create 1").unwrap();
        // Rebuild the root with an extra file next to meta and trees.
        let extra = oid_of(
            fx.git
                .run_stdin(["hash-object", "-w", "--stdin"], b"x")
                .unwrap(),
        );
        let meta = oid_of(fx.git.run(["rev-parse", &format!("{oid}:meta")]).unwrap());
        let trees = oid_of(fx.git.run(["rev-parse", &format!("{oid}:trees")]).unwrap());
        let root = format!(
            "100644 blob {meta}\tmeta\0040000 tree {trees}\ttrees\0100644 blob {extra}\textra\0"
        );
        let root = oid_of(fx.git.run_stdin(["mktree", "-z"], root.as_bytes()).unwrap());
        let commit = oid_of(fx.git.run(["commit-tree", &root, "-m", "x"]).unwrap());
        fx.git
            .run(["update-ref", &repo.scope.state_ref(), &commit])
            .unwrap();
        let err = State::load(&repo).unwrap_err();
        assert!(
            err.to_string().contains("unexpected entry `extra`"),
            "{err}"
        );
    }
}
