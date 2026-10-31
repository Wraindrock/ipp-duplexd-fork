#!/bin/sh
# The version is three numbers, and each one answers a different question:
#
#   1 . 4 . 27
#   |   |   `-- iteration: goes up every time a package is cut, nothing else.
#   |   `------ minor: a feature or a change a user would notice.
#   `---------- major: a structural change (the file format, the architecture,
#               something that has to be migrated).
#
# The rule is the usual one: raising a level resets the ones to its right to
# zero (1.4.27 -> minor -> 1.5.0), and a level may grow as far as it likes while
# the ones on its left stay put. Because every build gets its own iteration,
# two packages never share a version and "the newest one" is always well
# defined.
#
# The tag is the same string with a leading v (v1.4.27); Cargo.toml carries it
# without the v, and it is valid semver as it stands -- no suffixes, nothing to
# translate, nothing that can disagree with what the binaries report.
#
# Usage:
#   packaging/version.sh next              the next iteration (1.4.28)
#   packaging/version.sh minor             the next minor, iteration reset (1.5.0)
#   packaging/version.sh major             the next major, both reset (2.0.0)
#   packaging/version.sh set [version|next|minor|major]
#   packaging/version.sh check [tag]       the shape, and that a tag matches
set -eu
cd "$(dirname -- "$0")/.."

current() {
    sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1
}

# MAJOR.MINOR.ITERATION, e.g. 1.4.27
shape_ok() {
    printf '%s\n' "$1" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+$'
}

parts() {
    version=$1
    major=${version%%.*}
    rest=${version#*.}
    minor=${rest%%.*}
    iteration=${rest##*.}
}

next_iteration() {
    version=$(current)
    shape_ok "$version" ||
        { echo "Cargo.toml says '$version', which is not MAJOR.MINOR.ITERATION" >&2; exit 1; }
    parts "$version"
    printf '%s.%s.%s\n' "$major" "$minor" "$((iteration + 1))"
}

next_minor() {
    version=$(current)
    shape_ok "$version" ||
        { echo "Cargo.toml says '$version', which is not MAJOR.MINOR.ITERATION" >&2; exit 1; }
    parts "$version"
    printf '%s.%s.0\n' "$major" "$((minor + 1))"
}

next_major() {
    version=$(current)
    shape_ok "$version" ||
        { echo "Cargo.toml says '$version', which is not MAJOR.MINOR.ITERATION" >&2; exit 1; }
    parts "$version"
    printf '%s.0.0\n' "$((major + 1))"
}

write_version() {
    version=$1
    sed -i "s/^version = \".*\"/version = \"$version\"/" Cargo.toml
    # The lockfile lists the package itself; keep that line in step so `--locked`
    # builds do not fail on a version that no longer matches.
    line=$(grep -n '^name = "ipp-duplexd-fork"$' Cargo.lock | head -1 | cut -d: -f1)
    if [ -n "$line" ]; then
        sed -i "$((line + 1))s/.*/version = \"$version\"/" Cargo.lock
    fi
}

case "${1:-}" in
    next)
        next_iteration
        ;;
    minor)
        next_minor
        ;;
    major)
        next_major
        ;;
    set)
        case "${2:-}" in
            "") version=$(next_iteration) ;;
            next|iteration) version=$(next_iteration) ;;
            minor) version=$(next_minor) ;;
            major) version=$(next_major) ;;
            *) version=$2 ;;
        esac
        shape_ok "$version" ||
            { echo "'$version' is not a version like 1.4.27" >&2; exit 2; }
        write_version "$version"
        echo "version $version"
        ;;
    check)
        version=$(current)
        shape_ok "$version" ||
            { echo "Cargo.toml says '$version', which is not MAJOR.MINOR.ITERATION" >&2; exit 1; }
        if [ -n "${2:-}" ]; then
            if [ "$2" != "v$version" ]; then
                echo "tag $2 does not match Cargo.toml version v$version" >&2
                exit 1
            fi
        fi
        echo "version $version"
        ;;
    *)
        echo "usage: $0 {next|minor|major|set [version|next|minor|major]|check [tag]}" >&2
        exit 2
        ;;
esac
