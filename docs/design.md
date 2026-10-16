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
