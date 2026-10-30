#!/bin/sh
# The version has three parts:
#
#   2610 . 3 - 2
#   |    |   |
#   |    |   '-- fix counter: the 2nd fix (or change) for feature 3 and the
#   |    |       features before it. Omitted on the feature release itself.
#   |    '------ feature counter: starts at 1 each month and goes up by one
#   |            for every feature update, so 2610.3 is the third one in
#   |            October 2026.
#   '----------- year and month (YYMM) the feature shipped in: the release
#               train, which is what the major number used to be.
#
# A fix keeps the train and the feature it belongs to: 2610.3-1, 2610.3-2, and
# those sort after 2610.3 and before 2610.4. That is the whole point -- the
# number says *what* a build contains, not when it was typed.
#
# The tag is the same string with a leading v (v2610.3-1); Cargo.toml carries it
# without the v. The month comes from the local date, so honour TZ when cutting
# from another timezone: TZ=Asia/Shanghai packaging/version.sh next
#
# Usage:
#   packaging/version.sh next             the next feature release (YYMM.N)
#   packaging/version.sh fix              the next fix release (YYMM.N-x)
#   packaging/version.sh set [version]    write it to Cargo.toml and Cargo.lock
#   packaging/version.sh check [tag]      fail unless the shape is right and,
#                                         with a tag, `v<version>` is that tag
set -eu
cd "$(dirname -- "$0")/.."

current() {
    sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1
}

# Cargo needs three numbers, so the fix counter lives in the patch position:
# Cargo.toml says 2610.3.0 for the feature release and 2610.3.2 after two fixes.
# The tag (and everything a person reads) drops a zero patch and writes the
# counter after a dash instead: v2610.3, v2610.3-2.
shape_ok() {
    printf '%s\n' "$1" | grep -Eq '^[0-9]{4}\.[0-9]+\.[0-9]+$'
}

# 2610.3.2 -> 2610.3-2;  2610.3.0 -> 2610.3
as_tag() {
    version=$1
    major=${version%%.*}
    rest=${version#*.}
    feature=${rest%%.*}
    fix=${rest##*.}
    if [ "$fix" = "0" ]; then
        printf '%s.%s\n' "$major" "$feature"
    else
        printf '%s.%s-%s\n' "$major" "$feature" "$fix"
    fi
}

train() {
    date +%y%m
}

next_feature() {
    prefix=$(train)
    highest=$(git tag -l "v$prefix.*" 2>/dev/null |
        sed -n "s/^v$prefix\.\([0-9][0-9]*\)\(-[0-9][0-9]*\)\{0,1\}\$/\1/p" |
        sort -n | tail -1)
    printf '%s.%s.0\n' "$prefix" "$((${highest:-0} + 1))"
}

next_fix() {
    version=$(current)
    shape_ok "$version" ||
        { echo "Cargo.toml says '$version', which is not YYMM.N.X" >&2; exit 1; }
    base=${version%.*}
    last=${version##*.}
    printf '%s.%s\n' "$base" "$((last + 1))"
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
        next_feature
        ;;
    fix)
        next_fix
        ;;
    as-tag)
        as_tag "${2:-$(current)}"
        ;;
    set)
        case "${2:-}" in
            "") version=$(next_feature) ;;
            fix) version=$(next_fix) ;;
            *) version=$2 ;;
        esac
        shape_ok "$version" ||
            { echo "'$version' is not a version like 2610.3.0 or 2610.3.1" >&2; exit 2; }
        write_version "$version"
        echo "version $version"
        ;;
    check)
        version=$(current)
        shape_ok "$version" ||
            { echo "Cargo.toml says '$version', which is not YYMM.N.X" >&2; exit 1; }
        if [ -n "${2:-}" ]; then
            expected="v$(as_tag "$version")"
            if [ "$2" != "$expected" ]; then
                echo "tag $2 does not match Cargo.toml version $version (expected $expected)" >&2
                exit 1
            fi
        fi
        echo "version $version"
        ;;
    *)
        echo "usage: $0 {next|fix|set [version|fix]|check [tag]}" >&2
        exit 2
        ;;
esac
