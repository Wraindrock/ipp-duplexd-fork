#!/bin/bash
# Cut a release: give the tree its date-based version, run the CI checks,
# commit, tag and push — the Release workflow then builds the packages, the
# AppImage and the dnf repository.
#
# Usage: packaging/release.sh [version]
#   without a version: today's date and the next free counter (2026.10.26-2)
#   with a version:    that exact YYYY.MM.DD-N
# Needs: git, cargo, shellcheck; a clean working tree.
set -euo pipefail
cd "$(dirname "$0")/.."

if (($# > 1)); then
    echo "usage: $0 [version]    e.g. $0 2026.10.26-3" >&2
    exit 2
fi
if ! command -v shellcheck >/dev/null; then
    echo "shellcheck is not installed" >&2
    echo "  e.g. sudo pacman -S shellcheck  /  sudo apt install shellcheck" >&2
    exit 1
fi
# the release commit should hold nothing but the version bump
if [[ -n "$(git status --porcelain)" ]]; then
    echo "the working tree is not clean — commit or stash first" >&2
    exit 1
fi

version=${1:-$(./packaging/version.sh next)}
./packaging/version.sh set "$version"
version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
if git rev-parse -q --verify "refs/tags/v$version" >/dev/null; then
    echo "tag v$version already exists" >&2
    exit 1
fi

echo "==> checking $version"
# same commands as .github/workflows/ci.yml, warnings denied like there
export RUSTFLAGS="-D warnings"
cargo fmt --check
cargo clippy --all-targets --features gui -- -D warnings
cargo clippy --all-targets -- -D warnings
shellcheck packaging/*.sh
cargo build --all-targets --locked
cargo test --features gui --locked
./packaging/version.sh check "v$version"

echo "==> committing and tagging"
git -c user.name=ian -c user.email=ian@localhost add -A
git -c user.name=ian -c user.email=ian@localhost commit -m "Release v$version"
git -c user.name=ian -c user.email=ian@localhost tag -a "v$version" -m "v$version"

git log --oneline --decorate -1
read -r -p "push main and v$version to origin? [y/N] " answer
if [[ "$answer" != [yY] ]]; then
    echo "not pushed — when ready: git push origin main && git push origin v$version"
    exit 0
fi
git push origin main
git push origin "v$version"

echo
echo "done — the Release workflow is building the packages:"
echo "  https://github.com/Wraindrock/ipp-duplexd-fork/actions"
