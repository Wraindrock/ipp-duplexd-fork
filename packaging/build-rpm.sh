#!/bin/bash
# Build an RPM (Fedora / RHEL / openSUSE) from the working tree.
# Needs: cargo, rpmbuild. Result: packaging/ipp-duplexd-fork-<date>-<counter>[.<dist>].<arch>.rpm
set -euo pipefail
cd "$(dirname "$0")/.."

version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
if [[ -z "$version" ]]; then
    echo "no version in Cargo.toml" >&2
    exit 1
fi
# RPM has no room for a dash inside Version, so the project version
# YYYY.MM.DD-N (packaging/version.sh) is split into Version and Release.
ver=${version%%-*}
rel=${version##*-}
if [[ "$rel" == "$version" ]]; then
    rel=1
fi
if ! command -v rpmbuild >/dev/null; then
    echo "rpmbuild is not installed" >&2
    echo "  e.g. sudo dnf install rpm-build  /  sudo zypper install rpm-build" >&2
    exit 1
fi

top=$(mktemp -d)
trap 'rm -rf "$top"' EXIT
mkdir -p "$top/BUILD" "$top/BUILDROOT" "$top/RPMS" "$top/SOURCES" "$top/SPECS" "$top/SRPMS"

# same file list as the Arch tarball; locales/ is needed at compile time
tar czf "$top/SOURCES/ipp-duplexd-fork.tar.gz" \
    --transform "s,^,ipp-duplexd-fork/," \
    Cargo.toml Cargo.lock src locales docs README.md LICENSE \
    packaging/ipp-duplexd-fork.service packaging/ipp-duplexd-fork.conf.example

# --nodeps: the BuildRequires above are what a distribution build checks;
# this script must also work on a host without an RPM database (the same way
# build-deb.sh does not check its dependencies), and %build fails loudly if a
# tool is really missing.
rpmbuild -bb --nodeps \
    --define "_topdir $top" \
    --define "_sourcedir $top/SOURCES" \
    --define "_dbpath $top/db" \
    --define "_tmppath $top/tmp" \
    --define "ipp_version $ver" \
    --define "ipp_release $rel" \
    packaging/rpm/ipp-duplexd-fork.spec

built=$(find "$top/RPMS" -name '*.rpm' -print -quit)
if [[ -z "$built" ]]; then
    echo "rpmbuild produced no RPM" >&2
    exit 1
fi
out="packaging/$(basename "$built")"
cp "$built" "$out"

# Verify the package before keeping it: a green-looking artifact with a missing
# binary or unit is worse than a failed build. Lists are read once and matched
# with herestrings, because a "cmd | grep -q" under pipefail reports a false
# negative as soon as grep exits early.
# --dbpath: the queries must not need (or touch) the host's RPM database
mkdir -p "$top/db"
listing=$(rpm --dbpath "$top/db" -qpl "$out")
requires=$(rpm --dbpath "$top/db" -qpR "$out")
problems=()
has() { grep -Fxq -- "$1" <<<"$listing"; }
has /usr/bin/ipp-duplexd-fork || problems+=("no /usr/bin/ipp-duplexd-fork")
has /usr/bin/ipp-duplexd-fork-setup || problems+=("no /usr/bin/ipp-duplexd-fork-setup")
has /usr/bin/ipp-duplexd-fork-gui || problems+=("no /usr/bin/ipp-duplexd-fork-gui")
has /usr/lib/systemd/user/ipp-duplexd-fork.service || problems+=("no systemd user unit")
# Fedora puts docs in /usr/share/doc/ipp-duplexd-fork-<version>/, other layouts use
# a fixed /usr/share/doc/ipp-duplexd-fork/, so only the file name and the directory
# prefix are checked
grep -Eq '^/usr/share/doc/ipp-duplexd-fork[^/]*/README\.md$' <<<"$listing" \
    || problems+=("no README")
grep -Eq '^/usr/share/licenses/ipp-duplexd-fork[^/]*/LICENSE$' <<<"$listing" \
    || problems+=("no license")
grep -Fxq 'qpdf' <<<"$requires" || problems+=("no qpdf dependency")
grep -Fxq 'gtk4' <<<"$requires" || problems+=("no gtk4 dependency")
grep -Fxq 'libadwaita' <<<"$requires" || problems+=("no libadwaita dependency")
# the catalogs are compiled into the binaries; nothing should be installed for
# them, and if something is, the file list and the spec drifted apart
if grep -Eq 'locales|\.yml$' <<<"$listing"; then
    problems+=("locale data installed, but the catalogs are compiled in")
fi
if (( ${#problems[@]} )); then
    echo "the RPM at $out is incomplete:" >&2
    printf '  - %s\n' "${problems[@]}" >&2
    rm -f "$out"
    exit 1
fi

echo
while IFS= read -r member; do
    echo "  $member"
done <<<"$listing"
echo
echo "install with: sudo dnf install ./$out    (openSUSE: sudo zypper install ./$out)"
