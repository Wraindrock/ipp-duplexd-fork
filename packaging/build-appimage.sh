#!/usr/bin/env bash
# Build one AppImage for trying the fork without installing it.
#
# The image carries the daemon, the setup wizard and the GTK4 application, but
# *not* GTK4 or libadwaita: it uses the copies from the machine it runs on, so
# the window follows that desktop's own theme — on GNOME 49 it looks like
# GNOME 49, not like whatever archive happened to be used to build this file.
# That is also why it needs gtk4 >= 4.10 and libadwaita >= 1.5 installed (see
# the README); the host also needs qpdf for the daemon to print.
#
# It is a test build, not an alternative to the packages: it is unsigned, it is
# not registered with the desktop, and the setup wizard still needs the host's
# cups-client, because it must talk to the real CUPS server.
#
# Requires: cargo and the GTK4/libadwaita development files (see the README's
# "Building from source" section), plus network access the first time, to fetch
# the pinned AppImage tooling into ${APPIMAGE_TOOLS:-~/.cache/ipp-duplexd-fork/appimage}.
set -euo pipefail

repo=$(cd "$(dirname "$0")/.." && pwd)
cd "$repo"

version=$(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -1)
if [ -z "$version" ]; then
    echo "cannot read the version from Cargo.toml" >&2
    exit 1
fi

arch=${APPIMAGE_ARCH:-$(uname -m)}
if [ "$arch" != x86_64 ]; then
    # The pinned checksums below are for the x86_64 tools; aarch64 would need
    # its own entries rather than silently building something unverified.
    echo "only x86_64 AppImages are built (got '$arch')" >&2
    exit 1
fi

# Pinned by release tag (not "continuous") so the same tooling is used until
# somebody deliberately bumps it, and verified against a checksum so a moved
# tag or a tampered download fails loudly instead of shipping.
appimagetool_version=1.9.1
appimagetool_sha256=ed4ce84f0d9caff66f50bcca6ff6f35aae54ce8135408b3fa33abfc3cb384eb0

tools=${APPIMAGE_TOOLS:-${XDG_CACHE_HOME:-$HOME/.cache}/ipp-duplexd-fork/appimage}
mkdir -p "$tools"

fetch() { # url sha256 path
    local url=$1 sum=$2 path=$3
    if [ -f "$path" ] && printf '%s  %s\n' "$sum" "$path" | sha256sum -c --status; then
        return
    fi
    echo "downloading $url"
    curl -fsSL --retry 3 --retry-delay 2 -m 900 -o "$path.part" "$url"
    if ! printf '%s  %s\n' "$sum" "$path.part" | sha256sum -c --status; then
        echo "checksum mismatch for $url" >&2
        rm -f "$path.part"
        exit 1
    fi
    mv "$path.part" "$path"
}

fetch "https://github.com/AppImage/appimagetool/releases/download/$appimagetool_version/appimagetool-x86_64.AppImage" \
    "$appimagetool_sha256" "$tools/appimagetool"
chmod +x "$tools/appimagetool"

echo "building version $version for $arch"
cargo build --release --locked --features gui
target_dir=${CARGO_TARGET_DIR:-target}

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
appdir=$work/AppDir
mkdir -p "$appdir/usr/bin"

# The three programs are the whole payload: everything else (GTK, libadwaita,
# qpdf) comes from the machine, which keeps the image small and, more
# importantly, keeps the widget style in step with the desktop.
for binary in ipp-duplexd-fork ipp-duplexd-fork-gui ipp-duplexd-fork-setup; do
    install -m 0755 "$target_dir/release/$binary" "$appdir/usr/bin/$binary"
done
install -m 0755 packaging/appimage/AppRun "$appdir/AppRun"
install -m 0644 packaging/io.github.wraindrock.ipp_duplexd_fork.desktop \
    "$appdir/io.github.wraindrock.ipp_duplexd_fork.desktop"
install -m 0644 packaging/io.github.wraindrock.ipp_duplexd_fork.svg \
    "$appdir/io.github.wraindrock.ipp_duplexd_fork.svg"

# APPIMAGE_EXTRACT_AND_RUN keeps the tools usable on machines without FUSE
# (containers, minimal installs), which is also where this script tends to run.
export APPIMAGE_EXTRACT_AND_RUN=1

out=packaging/ipp-duplexd-fork-$version-$arch.AppImage
rm -f "$out"
ARCH="$arch" "$tools/appimagetool" "$appdir" "$out"
chmod +x "$out"

problems=()

# The contents: three binaries and nothing that would pin the image to an old
# toolkit. A bundled libgtk/libadwaita is exactly the mistake this layout is
# there to avoid, so it is an error rather than a nicety.
extracted=$work/extracted
mkdir -p "$extracted"
( cd "$extracted" && "$repo/$out" --appimage-extract >/dev/null )
root=$extracted/squashfs-root
for binary in ipp-duplexd-fork ipp-duplexd-fork-gui ipp-duplexd-fork-setup; do
    if [ ! -x "$root/usr/bin/$binary" ]; then
        problems+=("no /usr/bin/$binary")
    fi
done
for library in libgtk-4.so libadwaita-1.so; do
    if [ -n "$(find "$root/usr" -name "$library*" -print -quit 2>/dev/null)" ]; then
        problems+=("$library is bundled; the window must use the desktop's copy")
    fi
done
if [ ! -f "$root/io.github.wraindrock.ipp_duplexd_fork.desktop" ]; then
    problems+=("no desktop entry at the image root")
fi
if [ ! -f "$root/io.github.wraindrock.ipp_duplexd_fork.svg" ]; then
    problems+=("no application icon at the image root")
fi

# The behaviour: a portable image must still speak its languages, and the
# window must be able to open (Xvfb, if this machine has it).
help=$(./"$out" --help 2>&1) || problems+=("the dialog's --help failed")
case "$help" in
    *--flip*) ;;
    *) problems+=("the dialog's --help does not mention --flip") ;;
esac

version_out=$(./"$out" --version 2>&1) || problems+=("the dialog's --version failed")
case "$version_out" in
    *"$version"*) ;;
    *) problems+=("--version does not report $version") ;;
esac

code=0
answer=$(./"$out" daemon --lang zh-CN 2>&1) || code=$?
if [ "$code" -ne 2 ]; then
    problems+=("the daemon inside the image exited $code instead of 2 without --printer")
fi
case "$answer" in
    *必须指定*) ;;
    *) problems+=("the daemon inside the image did not speak Chinese") ;;
esac

if command -v xvfb-run >/dev/null; then
    if ! IPP_DUPLEXD_FORK_GUI_TEST=accept xvfb-run -a ./"$out" --flip "smoke [odd pages]" --lang zh-CN; then
        problems+=("the dialog did not confirm under Xvfb")
    fi

    if ! IPP_DUPLEXD_FORK_GUI_TEST=window xvfb-run -a ./"$out" --lang zh-CN; then
        problems+=("the window did not open under Xvfb")
    fi
else
    echo "note: xvfb-run is missing, so the window was not exercised"
fi

if [ ${#problems[@]} -ne 0 ]; then
    echo "the AppImage at $out is incomplete:" >&2
    for problem in "${problems[@]}"; do
        echo "  - $problem" >&2
    done
    rm -f "$out"
    exit 1
fi

size=$(du -h "$out" | cut -f1)
echo "built $out ($size)"
echo "try it with: ./$out            # the window"
echo "             ./$out daemon ...  # the daemon inside the image"
echo "             ./$out setup       # the setup wizard"
