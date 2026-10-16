#!/bin/sh
# Installs git-stone from the latest GitHub release.
#   curl -fsSL https://raw.githubusercontent.com/HarshKothari88/git-stone/main/install.sh | sh
# Options (environment): PREFIX (default ~/.local), VERSION (default: latest, e.g. v0.1.0).
set -eu

repo="HarshKothari88/git-stone"
prefix="${PREFIX:-$HOME/.local}"

case "$(uname -s)-$(uname -m)" in
  Linux-x86_64) target="x86_64-unknown-linux-gnu" ;;
  Linux-aarch64 | Linux-arm64) target="aarch64-unknown-linux-gnu" ;;
  Darwin-x86_64) target="x86_64-apple-darwin" ;;
  Darwin-arm64) target="aarch64-apple-darwin" ;;
  *) echo "git-stone: no prebuilt binary for $(uname -s) $(uname -m). Build it with: cargo install --git https://github.com/$repo" >&2; exit 1 ;;
esac

if [ -n "${VERSION:-}" ]; then
  base="https://github.com/$repo/releases/download/$VERSION"
else
  base="https://github.com/$repo/releases/latest/download"
fi
name="git-stone-$target"

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
cd "$tmp"
echo "Downloading $name ..."
curl -fsSL -O "$base/$name.tar.gz"
curl -fsSL -O "$base/$name.tar.gz.sha256"
if command -v sha256sum >/dev/null 2>&1; then sha256sum -c "$name.tar.gz.sha256" >/dev/null
else shasum -a 256 -c "$name.tar.gz.sha256" >/dev/null; fi
tar xzf "$name.tar.gz"

mkdir -p "$prefix/bin" "$prefix/share/man/man1"
install -m 755 "$name/git-stone" "$prefix/bin/git-stone"
install -m 644 "$name/git-stone.1" "$prefix/share/man/man1/git-stone.1"
echo "Installed $prefix/bin/git-stone"
case ":$PATH:" in *":$prefix/bin:"*) ;; *) echo "Add $prefix/bin to your PATH to use \`git stone\`." ;; esac
