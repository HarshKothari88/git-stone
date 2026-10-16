#!/usr/bin/env bash
# Times git-stone on repositories of different sizes. Not run in CI.
#
#   cargo build --release && bench/bench.sh            # 1k, 10k and 50k files, then 1k + a 100 MB binary
#   bench/bench.sh 1000 5000                           # your own sizes (file counts)
#
# Environment:
#   GIT_STONE   path to the binary (default: target/release/git-stone)
#   RUNS        runs per read command (default: 5); the median is printed
#   STONES      stones to build before timing (default: 100)
#   KEEP=1      keep the generated repositories (their paths are printed)
#
# `hyperfine` is not needed: a small timing loop (perl, which ships with macOS and Linux) is used.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
GIT_STONE="${GIT_STONE:-$here/target/release/git-stone}"
RUNS="${RUNS:-5}"
STONES="${STONES:-100}"
[ -x "$GIT_STONE" ] || { echo "build first: cargo build --release (looked for $GIT_STONE)" >&2; exit 1; }

work="$(mktemp -d "${TMPDIR:-/tmp}/git-stone-bench.XXXXXX")"
[ "${KEEP:-0}" = 1 ] || trap 'rm -rf "$work"' EXIT
# Git finds the binary the way a user would: as `git-stone` on PATH.
mkdir -p "$work/bin"
ln -s "$GIT_STONE" "$work/bin/git-stone"
export PATH="$work/bin:$PATH"
export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1
export GIT_AUTHOR_NAME=Bench GIT_AUTHOR_EMAIL=bench@example.com
export GIT_COMMITTER_NAME=Bench GIT_COMMITTER_EMAIL=bench@example.com

# Median of RUNS runs.
median() {
  local times=()
  for _ in $(seq "$RUNS"); do times+=("$(timed "$@")"); done
  printf '%s\n' "${times[@]}" | sort -n | awk '{a[NR]=$1} END {print a[int((NR+1)/2)]}'
}

# One run of a command: prints its time in milliseconds (its own output is thrown away); FAILED if it fails.
timed() {
  perl -MTime::HiRes=time -e '
    my $t = time;
    open my $saved, ">&", \*STDOUT; open STDOUT, ">", "/dev/null"; open STDERR, ">", "/dev/null";
    my $rc = system(@ARGV); my $dt = (time - $t) * 1000;
    open STDOUT, ">&", $saved;
    if ($rc) { print STDOUT "FAILED\n"; exit 1 }
    printf "%.1f\n", $dt;' -- "$@"
}

row() { printf '%-9s %-26s %10s ms\n' "$1" "$2" "$3"; }

# Build a repo: $1 files in 100 directories, ~1 KB each; $2 = MB of one random binary file (0 = none).
make_repo() {
  local files="$1" big="$2" dir="$3"
  mkdir -p "$dir" && cd "$dir"
  git init -q -b main .
  perl -e '
    my ($n) = @ARGV;
    for my $i (0 .. $n - 1) {
      my $d = sprintf "d%02d", $i % 100;
      mkdir $d unless -d $d;
      open my $f, ">", sprintf("%s/f%d.txt", $d, $i) or die;
      print $f "file $i\n" . ("line of text for the benchmark\n" x 30);
    }' "$files"
  if [ "$big" -gt 0 ]; then head -c "$((big * 1024 * 1024))" /dev/urandom > big.bin; fi
  git add -A && git commit -q -m base
}

bench_repo() {
  local label="$1" files="$2" big="$3" dir="$work/$1"
  echo
  if [ "$big" -gt 0 ]; then echo "== $label ($files files + one ${big} MB binary) =="; else echo "== $label ($files files) =="; fi
  make_repo "$files" "$big" "$dir"
  cd "$dir"

  # create: $STONES stones, each after touching 10 files. Median of the creates.
  local creates=()
  for i in $(seq "$STONES"); do
    for j in $(seq 0 9); do
      f="d$(printf '%02d' $(((i * 10 + j) % 100)))/f$(((i * 10 + j) % files)).txt"
      echo "edit $i" >> "$f"
    done
    creates+=("$(timed git stone "stone $i")")
  done
  local create_ms
  create_ms="$(printf '%s\n' "${creates[@]}" | sort -n | awk '{a[NR]=$1} END {print a[int((NR+1)/2)]}')"
  # Something to show as "current changes".
  for j in 1 2 3; do echo "unstoned" >> "d0$j/f$j.txt"; done

  local base status list review diffc diffn
  base="$(median git status --porcelain)"
  status="$(median git stone status)"
  list="$(median git stone list)"
  review="$(median git stone review)"
  diffc="$(median git stone diff current)"
  diffn="$(median git stone diff "$((STONES / 2))")"

  row "$label" "git status (baseline)" "$base"
  row "$label" "create (median of $STONES)" "$create_ms"
  row "$label" "status" "$status"
  row "$label" "list" "$list"
  row "$label" "review ($STONES stones)" "$review"
  row "$label" "diff current" "$diffc"
  row "$label" "diff <middle stone>" "$diffn"

  # drop and commit change the chain, so they run last, one stone each time.
  local drops=() commits=()
  for _ in $(seq "$RUNS"); do
    id="$(git stone list --porcelain | tr '\0' '\n' | awk -F'\t' '$1=="stone"{id=$2} END{print id}')"
    drops+=("$(timed git stone drop "$id")")
  done
  for _ in $(seq "$RUNS"); do
    first="$(git stone list --porcelain | tr '\0' '\n' | awk -F'\t' '$1=="stone"{print $2; exit}')"
    commits+=("$(timed git stone "$first" commit -m bench --no-verify)")
  done
  row "$label" "drop (median of $RUNS)" "$(printf '%s\n' "${drops[@]}" | sort -n | awk '{a[NR]=$1} END {print a[int((NR+1)/2)]}')"
  row "$label" "commit (median of $RUNS)" "$(printf '%s\n' "${commits[@]}" | sort -n | awk '{a[NR]=$1} END {print a[int((NR+1)/2)]}')"

  # The targets of the plan, as ratios and limits.
  awk -v c="$create_ms" -v s="$status" -v b="$base" -v l="$list" -v r="$review" -v lab="$label" 'BEGIN {
    printf "%-9s %-26s %s\n", lab, "create / git status", sprintf("%.2fx (target <= 2x)", c / b);
    printf "%-9s %-26s %s\n", lab, "status / git status", sprintf("%.2fx (target <= 2x)", s / b);
    printf "%-9s %-26s %s\n", lab, "list", sprintf("%s ms (target < 50 ms)", l);
    printf "%-9s %-26s %s\n", lab, "review", sprintf("%s ms (target < 2000 ms)", r);
  }'
  cd "$work"
}

echo "git-stone: $("$GIT_STONE" version)   git: $(git --version)   runs: $RUNS   stones: $STONES"
if [ "$#" -gt 0 ]; then
  for n in "$@"; do bench_repo "${n}f" "$n" 0; done
else
  for n in 1000 10000 50000; do bench_repo "${n}f" "$n" 0; done
  bench_repo "1000f+100MB" 1000 100
fi
[ "${KEEP:-0}" = 1 ] && echo && echo "kept: $work"
exit 0
