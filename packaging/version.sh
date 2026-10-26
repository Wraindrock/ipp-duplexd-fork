#!/bin/sh
# The project version is the day a build is cut plus how many builds were cut
# that day: 2026.10.26-1, 2026.10.26-2, ... The tag is the same string with a
# leading v (v2026.10.26-3); Cargo.toml carries it without the v.
#
# The counter is not decoration: it keeps two builds of one day apart and makes
# them sort in the order they were cut, so "the newest build wins" also holds
# when several land on a single day. The date is the local date at the moment
# the release is cut, so honour TZ when cutting from another timezone:
# TZ=Asia/Shanghai packaging/version.sh next
#
# Usage:
#   packaging/version.sh next             the version today's next build gets
#   packaging/version.sh set [version]    write it to Cargo.toml and Cargo.lock
#   packaging/version.sh check [tag]      fail unless the shape is right and,
#                                         with a tag, `v<version>` is that tag
set -eu
cd "$(dirname -- "$0")/.."

current() {
    sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1
}

# vYYYY.MM.DD-N, e.g. v2026.10.26-3
shape_ok() {
    printf '%s\n' "$1" | grep -Eq '^[0-9]{4}\.[0-9]{2}\.[0-9]{2}-[0-9]+$'
}

today() {
    date +%Y.%m.%d
}

next_version() {
    day=$(today)
    used=$(git tag -l "v$day-*" 2>/dev/null | wc -l | tr -d ' ')
    printf '%s-%s\n' "$day" "$((used + 1))"
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
        next_version
        ;;
    set)
        version=${2:-$(next_version)}
        shape_ok "$version" ||
            { echo "'$version' is not a version like 2026.10.26-3" >&2; exit 2; }
        write_version "$version"
        echo "version $version"
        ;;
    check)
        version=$(current)
        shape_ok "$version" ||
            { echo "Cargo.toml says '$version', which is not YYYY.MM.DD-N" >&2; exit 1; }
        if [ -n "${2:-}" ]; then
            if [ "$2" != "v$version" ]; then
                echo "tag $2 does not match Cargo.toml version v$version" >&2
                exit 1
            fi
        fi
        echo "version $version"
        ;;
    *)
        echo "usage: $0 {next|set [version]|check [tag]}" >&2
        exit 2
        ;;
esac
