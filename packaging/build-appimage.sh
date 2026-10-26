#!/usr/bin/env bash
# Build one self-contained AppImage for trying the fork without installing it.
#
# The image carries the daemon, the setup wizard and the GTK4 dialog, plus the
# GTK4, libadwaita and qpdf libraries they need, the gdk-pixbuf loaders, the
# GSettings schemas and the GTK immodules. That makes it useful for a quick
# test on a machine where nothing of this is installed.
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
linuxdeploy_version=1-alpha-20251107-1
linuxdeploy_sha256=c20cd71e3a4e3b80c3483cef793cda3f4e990aca14014d23c544ca3ce1270b4d
appimagetool_version=1.9.1
appimagetool_sha256=ed4ce84f0d9caff66f50bcca6ff6f35aae54ce8135408b3fa33abfc3cb384eb0
# The GTK plugin publishes no releases, so pin the commit instead.
gtk_plugin_commit=7a3fbc31a9e5075073ff8790f26effbac5f84453
gtk_plugin_sha256=b0f4cbc684a0103a9651f0955b635eaea0096b3a66c0f5a2c2aa337960375171

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

fetch "https://github.com/linuxdeploy/linuxdeploy/releases/download/$linuxdeploy_version/linuxdeploy-x86_64.AppImage" \
    "$linuxdeploy_sha256" "$tools/linuxdeploy"
fetch "https://github.com/AppImage/appimagetool/releases/download/$appimagetool_version/appimagetool-x86_64.AppImage" \
    "$appimagetool_sha256" "$tools/appimagetool"
fetch "https://raw.githubusercontent.com/linuxdeploy/linuxdeploy-plugin-gtk/$gtk_plugin_commit/linuxdeploy-plugin-gtk.sh" \
    "$gtk_plugin_sha256" "$tools/linuxdeploy-plugin-gtk.sh"
chmod +x "$tools/linuxdeploy" "$tools/appimagetool" "$tools/linuxdeploy-plugin-gtk.sh"

echo "building version $version for $arch"
cargo build --release --locked --features gui
target_dir=${CARGO_TARGET_DIR:-target}

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
appdir=$work/AppDir
mkdir -p "$appdir"

# APPIMAGE_EXTRACT_AND_RUN keeps the tools usable on machines without FUSE
# (containers, minimal installs), which is also where this script tends to run.
export APPIMAGE_EXTRACT_AND_RUN=1
export DEPLOY_GTK_VERSION=4
# The libraries archived from a distribution are stripped already, so strip has
# nothing to win; and the copy bundled with linuxdeploy is older than the
# binutils that produce .relr.dyn sections, so it would reject them as
# "unknown type [0x13]" and abort the run mid-way.
export NO_STRIP=1
PATH="$tools:$PATH"
export PATH

executables=(
    --executable "$target_dir/release/ipp-duplexd-fork"
    --executable "$target_dir/release/ipp-duplexd-fork-gui"
    --executable "$target_dir/release/ipp-duplexd-fork-setup"
    --desktop-file packaging/appimage/ipp-duplexd-fork.desktop
    --icon-file packaging/appimage/ipp-duplexd-fork.svg
    --plugin gtk
)
qpdf_bin=$(command -v qpdf || true)
if [ -n "$qpdf_bin" ]; then
    executables+=(--executable "$qpdf_bin")
else
    echo "warning: qpdf is not installed here, so printing from the image needs the host's qpdf" >&2
fi

"$tools/linuxdeploy" --appdir "$appdir" "${executables[@]}"

# linuxdeploy generates an AppRun that starts a single executable; this image
# has three, so install the dispatching one (it still sources the GTK plugin's
# apprun-hooks, which is where the bundled loaders and schemas are announced).
install -m 0755 packaging/appimage/AppRun "$appdir/AppRun"

out=packaging/ipp-duplexd-fork-$version-$arch.AppImage
rm -f "$out"
ARCH="$arch" "$tools/appimagetool" "$appdir" "$out"
chmod +x "$out"

problems=()

# The contents: three binaries, and the two libraries that make the dialog a
# libadwaita application rather than a link error.
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
    if [ -z "$(find "$root/usr" -name "$library*" -print -quit)" ]; then
        problems+=("$library is not bundled")
    fi
done

# The behaviour: a self-contained image must still speak its languages, and the
# dialog must be able to open a window (Xvfb, if this machine has it).
help=$(./"$out" --help 2>&1) || problems+=("the dialog's --help failed")
case "$help" in
    *--flip*) ;;
    *) problems+=("the dialog's --help does not mention --flip") ;;
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
else
    echo "note: xvfb-run is missing, so the bundled GTK was not exercised"
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
echo "try it with: ./$out            # the flip dialog"
echo "             ./$out daemon ...  # the daemon inside the image"
echo "             ./$out setup       # the setup wizard"
