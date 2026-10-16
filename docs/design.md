# Git Stone design notes

Part A of the plan is copied here in Phase 9. Until then this file holds the findings from every
**VERIFY** item, checked on Git 2.53.0 (macOS). Each finding names the test that keeps it true.

## Phase 2 findings (snapshot, A5)

| # | Item | Finding | Test |
|---|------|---------|------|
| 1 | `git rev-parse --git-path index` honours a user-set `GIT_INDEX_FILE` | Yes. It prints the custom path (made absolute with `--path-format=absolute`, symlinks resolved). `Repo::index_path` therefore follows it, and the snapshot copies that file. | `git_index_file_is_what_git_path_reports` |
| 2 | `git add -A` in a sparse checkout | Does not error and does not drop out-of-cone entries. The snapshot holds the out-of-cone files exactly as the index has them. Works with a full index and with `--sparse-index`. | `sparse_checkout_keeps_out_of_cone_entries`, `sparse_checkout_with_sparse_index` |
| 3 | Split index | A temp copy placed in the worktree's git dir finds its `sharedindex.*` file. The tree equals the reference. | `split_index` |
| 4 | Linked worktree | `Repo::git_dir` and `index_path` are per worktree. The temp index is created in that worktree's git dir; the main worktree's index is untouched. | `linked_worktree_uses_its_own_index` |
| 5 | `post-index-change` hook | It fires on the temp index, and fires twice per snapshot (once for `add -A`, once for `write-tree`, which writes the cache-tree extension). `$GIT_INDEX_FILE` is the temp path. Harmless unless the hook assumes the real index. Document for users. | manual check (hook script writing a log) |
| 6 | Embedded git repository (not a registered submodule) | `git add -A` succeeds with exit 0 and prints an `embedded git repository` warning on stderr. It is recorded as a gitlink. We capture stderr and show nothing on success. | manual check |

### Behaviour the tests pin down (inherited from `git add -A`)

- Ignored files are absent from the snapshot.
- Assume-unchanged and skip-worktree files keep the index version, even if the file on disk changed.
- Staged and unstaged edits to one file: the working-tree version wins; the real index keeps the staged one.
- A submodule is recorded at its checked-out commit; uncommitted files inside it are not captured.
- CRLF is converted by `core.autocrlf=true` and by `.gitattributes` `text=auto`, exactly as `git add` would.
- Intent-to-add entries become real entries in the snapshot.
- File names with spaces, tab, newline, Unicode, a leading dash and shell syntax are stored exactly.
- A held `index.lock` on the real index does not matter (the temp copy has its own lock file).
- With no real index file (fresh repo), the snapshot works and does not create one.
- A corrupt real index makes the snapshot fail (exit 2) and leaves the real index and the git dir as they were.

### Implementation note

Temp index names are `stone-index-<pid>-<nanos>-<counter>`. The plan names `<pid>-<nanos>`; the counter
stops two snapshots in one process from sharing a file when they start in the same nanosecond.

## Phase 1 findings (state model)

- `git commit-tree` ignores `commit.gpgSign`, so saving a state commit never asks for a signature.
  Test: `save_ignores_signing_config`.
- State commits use a fixed author and committer (`git-stone <git-stone@localhost>`), so saving works
  when the user has no identity configured.
- `git mktree` checks that every referenced object exists, so a bad stone tree is caught at save time.
- `git ls-tree` orders `trees/` entries as text (`1`, `10`, `2`), so the loader never relies on entry order.
- Descriptions live in `desc/<id>` blobs. They are read in one `git cat-file --batch` call.

## Phase 5 findings (commit, auto-reconcile)

| # | Item | Finding | Test |
|---|------|---------|------|
| 1 | `read-tree -m <H0> <S>` from an unborn branch (A8) | Works with the empty tree as `<H0>`. The first commit is a root commit. | `unborn_branch_first_commit_is_a_root_commit` |
| 2 | **`read-tree -m` needs `-i` (design change)** | Without `-i`, a two-tree merge checks the working tree. Entries that an earlier `read-tree -m` wrote have no file stamps, so a later merge that changes such an entry says `Entry 'x' not uptodate. Cannot merge.` This hits the main use: Stone 2 edits a file Stone 1 created, and the user has edited it further. With `-i` the working tree is never looked at (fitting: we never touch it), and real clashes between staged changes and the stone are still found (`would be overwritten by merge`). The plan's A8.2, A8.5 and A9 now use `read-tree -m -i`. | `a_stone_that_builds_on_an_earlier_stone_commits_cleanly`, `staged_work_that_clashes_is_refused_before_anything_changes` |
| 3 | `git commit` against a temp `GIT_INDEX_FILE` | Hooks, signing, identity, editor, `-F` all work. `pre-commit` sees the temp index in `$GIT_INDEX_FILE`; a hook that runs `git add` changes the commit (we warn). | `a_hook_that_adds_files_gets_a_warning_and_the_chain_stays_consistent`, `signing_is_honoured` |
| 4 | `git log --first-parent --reverse -n N` | `-n` is applied before `--reverse`: it keeps the newest N commits, oldest first. A window that does not start on the anchor fails the parent check, so a long history can never be mistaken for a match. | `other_movements_are_not_reconciled` |
| 5 | `git commit` output | Git prints its own `[main abc1234] subject` summary on stdout before ours. | `commit_all_three_stones_in_order` |
| 6 | Relative `-F file` | `git commit` runs in the user's own directory, so a relative path means what they expect (the rest of the tool runs at the top of the working tree). | `author_date_signoff_and_message_files_are_passed_to_git` |

### Known gaps

- A real `index.lock` taken by another process between our check and `read-tree` is not simulated; the failure path prints the exact `git read-tree -m -i <H0> HEAD` recovery command.
- `git commit` of an empty stone (made with `--allow-empty`) fails with git's own "nothing to commit"; nothing is changed. Drop the stone or commit with a real change.

## Phase 6 findings (repair, adopt)

| # | Item | Finding | Test |
|---|------|---------|------|
| 1 | `git merge-tree --write-tree --merge-base` (A9) | Needs Git 2.40 or newer. `repair` checks this (`git version`) and nothing else does. | `git_versions_are_read_from_the_first_line` |
| 2 | Inputs | Plain **trees** work for the merge base and both sides. The plan's commit wrapping (`commit-tree`) is not needed, so no dangling commits are written. The empty tree works as the base (anchor `none`). | `an_anchor_of_none_uses_the_empty_tree_as_the_old_base` |
| 3 | Exit codes | 0 clean, 1 conflict, 128 for a bad object. `Git::run_answers` treats 1 as an answer and anything else as a failure. | `a_conflicting_replay_changes_nothing_and_names_the_stone_and_paths` |
| 4 | Output with `--name-only -z` | `<tree>\0`. On a conflict: each conflicted path, an empty token, then messages (ignored). The tree is written in both cases. | `merge_tree_output_is_split_into_tree_and_conflicted_paths` |
| 5 | Moving a chain (A10b) | `update-ref --stdin` with `start`, `update`, `delete`, `prepare`, `commit` is one transaction. Both old values are checked, so a changed source or target fails the whole thing and nothing moves. A reflog for the new ref needs `-c core.logAllRefUpdates=always` (custom namespaces get none by default). Deleting the source also deletes its reflog. | `save_moved_changes_both_refs_or_neither`, `detached_stones_follow_a_new_branch` |

### Behaviour the tests pin down

- The plan (no `--apply`) writes only unreferenced objects; refs, index and files stay as they were.
- After `repair --apply` every stone keeps its diff against the previous stone; only the base moves.
- A stone that is partly committed by hand shrinks to the rest. A rebase with `--autostash` followed by repair
  and commit does not revert the upstream change. A branch reset backwards keeps the stone's own work.
- A replay conflict names the first stone that conflicts and its paths, and changes nothing.
- `repair --apply` is refused while a merge or rebase runs, or when the branch has no commits. The plan still works.

### Known gaps

- **A conflicting replay is a dead end.** `drop` refuses when the branch moved (A9), so a stone that cannot be
  replayed cannot be dropped either. Open question for the reviewer.
- `repair` and `adopt` were not run on a SHA-256 repository; the state commit round trip is (Phase 1).
- A crash inside the `adopt` transaction is not simulated; git's `prepare` step makes it all-or-nothing.

## Phase 7 findings (interaction matrix)

| # | Item | Finding | Test |
|---|------|---------|------|
| 1 | **A4 VERIFY: `refs/worktree/*` and `git gc` (FAILED, design changed)** | `refs/worktree/stones/detached` is per worktree, but `git gc` run in a *different* worktree does not treat it as a root. Once its reflog is gone the detached chain is collected and `git stone` reports `stone state is corrupt`. (Kept while the reflog lives, and when gc runs in the owning worktree.) Reproduced on Git 2.53.0. **Fix: a detached chain lives in a shared ref**: `refs/stones/detached/main` for the main worktree, `refs/stones/detached/linked/<name>` for a linked one (`<name>` = its directory under `.git/worktrees/`, with every character outside `A-Za-z0-9_-` written as `%XX`). Shared refs are gc roots for every worktree. | `a_detached_chain_survives_gc_run_in_another_worktree_after_reflog_expiry`, `a_detached_chain_survives_gc_in_its_own_worktree_and_with_reflogs`, `worktree_ids_are_safe_inside_ref_names` |
| 2 | gc and dropped stones | After `drop`, the old state stays reachable through the reflog. Once the reflog is expired and `gc --prune=now` runs, the dropped stone's tree and `desc/` blob are gone and the live stone's tree stays. | `gc_collects_dropped_stones_but_keeps_live_ones` |
| 3 | Test clock | The test fixture fixes `GIT_COMMITTER_DATE` in the past, so reflog entries look old and a plain `gc` would expire them. Tests that need the reflog to count pass `-c gc.reflogExpire=never`. | same |

### Behaviour the tests pin down

- `add`, `restore`, `stash` (`-u`, `pop`) never change the state ref; status recomputes.
- Amend, reset, rebase, pull, merge, cherry-pick, am and a branch deleted and recreated elsewhere are all "moved": status warns, changes are refused, `list` still works, `repair --apply` fixes it.
- A conflicted cherry-pick: `create`, `commit` and `commit-all` refuse; `status`, `list`, `show` and `drop` work. A stopped rebase (detached HEAD) refuses too, and the chain is back after `rebase --abort`.
- `git reset --hard` plus `git clean`: the files are gone, `git checkout refs/stones/heads/<branch>:trees/<id> -- .` brings them back.
- Linked worktrees never see each other's stones; one branch in two worktrees is a warning.
- Failures: an unwritable object store gives exit 2, no state change, no temp index. An unwritable `refs/stones/heads` does the same for `create` and `drop`. A state ref pointing at an ordinary commit gives `stone state is corrupt` (exit 2) for every command and changes nothing. A temp index older than one hour is removed at the start of a changing command; a younger one is left alone.

### Consequences of the shared detached ref

- A worktree is told apart by its git dir, not its path. `git worktree remove` no longer deletes its detached chain; the chain stays under `refs/stones/detached/linked/<name>` until someone deletes that ref. Adding a worktree with the same name later finds the old chain; the usual movement rules (A9) then apply.
- Chains made by an earlier build under `refs/worktree/stones/detached` are not read. Nothing was released, so there is no migration.

### Known gaps

- **Disk full is simulated with read-only directories, not a failpoint.** The plan asked for a failpoint. Git's own write errors take the same path (a failed `git` call, exit 2), and a read-only object store makes a real write fail, so the error handling that runs is the real one. A true full disk (a write that fails halfway through a file) is not reproduced.
- `repair` with nothing to repair (in sync) exits 0 even during a merge; A11 says "repair refuses". It only refuses when there is something to change (`--apply` with a moved branch). Nothing can be lost either way.

## Prune (added after Phase 7)

`git stone prune [--apply] [--chain <ref>]` cleans up what nothing can reach any more.

- **Leftover** = a `refs/stones/heads/<b>` whose branch `refs/heads/<b>` is gone, or a `refs/stones/detached/linked/<name>` whose directory under `<common dir>/worktrees/` is gone. The chain of the current scope and `refs/stones/detached/main` are never leftovers.
- Without `--chain`, `--apply` removes only leftovers with **no stones**, plus temp indexes older than one hour. Chains that still hold stones (or cannot be read) are listed, with the exact `--chain` command and, for branches, the `adopt` hint. `--chain` takes the full ref, because a branch can be called `linked/x`.
- Each removal is `update-ref -d <ref> <old>`, so a chain that changed meanwhile is not deleted.
- **Deleting a ref also deletes its reflog**, so the old state cannot be found through the reflog. The command prints `git update-ref <ref> <oid>` for every removed chain; that works until `git gc` prunes the unreachable commit (two weeks by default).
- Empty chains of branches that still exist are left alone: removing them would restart ids at 1 (I7).
- Known gap: a worktree whose directory was deleted by hand but not yet pruned with `git worktree prune` still counts as existing.

Tests: `tests/prune.rs`.

## Phase 8 findings (benchmarks)

Run with `cargo build --release && bench/bench.sh` on an Apple M5 (10 cores), macOS, Git 2.53.0, one run, medians.
Repositories: N files of about 1 KB in 100 directories. 100 stones, each after changing 10 files. `list`,
`review` and the other reads are the median of 5 runs; `create` is the median of the 100 creates.

| Command | 1k files | 10k files | 50k files | 1k + 100 MB binary | Target (50k files) |
|---------|---------:|----------:|----------:|-------------------:|--------------------|
| `git status` (baseline) | 8.8 ms | 17.3 ms | 54.5 ms | 8.7 ms | |
| `create` | 122.9 ms | 121.9 ms | **206.7 ms** | 110.8 ms | within 2× of `git status` (**missed: 3.8×**) |
| `status` | 102.0 ms | 120.6 ms | **183.6 ms** | 116.2 ms | within 2× of `git status` (**missed: 3.4×**) |
| `list` | 31.3 ms | 31.9 ms | 31.5 ms | 35.4 ms | under 50 ms (met) |
| `review` (100 stones) | 552.5 ms | 565.2 ms | 749.6 ms | 617.9 ms | under 2 s (met) |
| `diff current` | 84.7 ms | 99.7 ms | 163.1 ms | 84.6 ms | |
| `diff <middle stone>` | 35.8 ms | 36.1 ms | 36.9 ms | 36.2 ms | |
| `drop` | 49.5 ms | 49.8 ms | 51.0 ms | 49.6 ms | |
| `commit` | 118.2 ms | 242.9 ms → **123.1 ms** | 4361.8 ms → **232.6 ms** | 273.0 ms | no target (outlier, fixed below) |

### Where the time goes (measured at 50k files)

- **Fixed cost.** One `git` process costs about 4 ms here. `list` is about 8 of them (31 ms), `drop` about 12 (50 ms) and
  `create`/`status` about 15 plus the snapshot. That is why `create` and `status` are 11–14× `git status` on a 1k-file
  repo (a 9 ms baseline) but 3.4–3.8× on 50k files: the fixed part stops mattering as the repo grows.
- **The snapshot.** `git add -A` on the temp index copy is 71 ms and `git write-tree` is 86 ms (the cache-tree is
  rebuilt for the whole index), about 157 ms together against 54 ms for `git status`. This is the cause of the missed 2×.
- **Large binary.** A 100 MB file that has not changed costs nothing (create 110.8 ms vs 122.9 ms at 1k files): the stat cache skips it.
- **Review and list** scale with the number of stones and changed paths, not with repository size, as designed (A13).

### Outlier: `commit` at 50k files took 4.4 s (fixed)

`git commit` ran against a temp index made by `git read-tree <stone tree>`. That index holds no file stat data, so
`git commit` re-hashed every file in the working tree (50k reads). Measured in the same repository:

| Index given to `git commit --allow-empty` | Time |
|---|---:|
| fresh `git read-tree <tree>` (what A8 did) | 3953 ms |
| the same with `--no-status` | 4433 ms |
| a copy of the real index (has stat data) | 54 ms |

**Fix:** the commit index is now a copy of the real index moved onto the stone with a single-tree merge,
`git read-tree -m -i <stone tree>`. A single-tree merge makes the index equal to the tree (staged work that is not in
the stone is dropped from this copy) but keeps the stat data of entries that already match. Before committing,
`git write-tree` on that index must equal the stone's tree, or the command stops with nothing changed (I5).
The user's own index is not touched by this step. After the fix: 50k files 232.6 ms (was 4361.8 ms), 10k files 123.1 ms (was 242.9 ms).

Test: `staged_work_never_leaks_into_the_commit_and_stays_staged_after_it` (a staged new file, a staged deletion and an
intent-to-add entry are absent from the commit and still staged afterwards), plus all existing commit tests.

### Targets summary

| Target | Result |
|---|---|
| `create` within 2× of `git status` at 50k files | missed, 3.8× |
| `status` within 2× of `git status` at 50k files | missed, 3.4× |
| `list` under 50 ms | met, 31.5 ms |
| `review` with 100 stones under 2 s | met, 749.6 ms |
