#!/bin/bash
# Build an Arch Linux package from the working tree.
# Result: packaging/arch/ipp-duplexd-fork-<date>-<counter>-<arch>.pkg.tar.zst
set -euo pipefail
cd "$(dirname "$0")/.."

# only for the closing hint; the PKGBUILD reads Cargo.toml for pkgver itself
ver=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)

# leftovers from when the tarball name carried the version
rm -f packaging/arch/ipp-duplexd-fork-*.tar.gz

tar czf "packaging/arch/ipp-duplexd-fork.tar.gz" \
    --transform "s,^,ipp-duplexd-fork/," \
    Cargo.toml Cargo.lock src locales docs README.md LICENSE \
    packaging/ipp-duplexd-fork.service packaging/ipp-duplexd-fork.conf.example

cd packaging/arch
makepkg -f -C "$@"
echo
echo "install with: sudo pacman -U packaging/arch/ipp-duplexd-fork-$ver-*.pkg.tar.zst"
