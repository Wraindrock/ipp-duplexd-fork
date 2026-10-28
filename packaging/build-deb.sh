#!/bin/bash
# Build a Debian/Ubuntu binary package from the working tree.
# Needs: cargo, dpkg-deb. Result: packaging/ipp-duplexd-fork_<version>_<arch>.deb
# (<version> is YYYY.MM.DD-N, see packaging/version.sh; Debian keeps it whole.)
set -euo pipefail
cd "$(dirname "$0")/.."

# already YYYY.MM.DD-N, which Debian reads as upstream-version/revision
ver=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
arch=$(dpkg --print-architecture 2>/dev/null || echo amd64)

cargo build --release --locked --features gui

stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT
chmod 755 "$stage"

install -Dm755 target/release/ipp-duplexd-fork "$stage/usr/bin/ipp-duplexd-fork"
install -Dm755 target/release/ipp-duplexd-fork-setup "$stage/usr/bin/ipp-duplexd-fork-setup"
install -Dm755 target/release/ipp-duplexd-fork-gui "$stage/usr/bin/ipp-duplexd-fork-gui"
install -Dm644 packaging/ipp-duplexd-fork.service "$stage/usr/lib/systemd/user/ipp-duplexd-fork.service"
install -Dm644 packaging/io.github.wraindrock.ipp_duplexd_fork.desktop \
    "$stage/usr/share/applications/io.github.wraindrock.ipp_duplexd_fork.desktop"
install -Dm644 packaging/io.github.wraindrock.ipp_duplexd_fork.svg \
    "$stage/usr/share/icons/hicolor/scalable/apps/io.github.wraindrock.ipp_duplexd_fork.svg"
install -Dm644 packaging/ipp-duplexd-fork.conf.example "$stage/usr/share/doc/ipp-duplexd-fork/ipp-duplexd-fork.conf.example"
install -Dm644 README.md "$stage/usr/share/doc/ipp-duplexd-fork/README.md"

# debian copyright file from LICENSE
install -Dm644 /dev/stdin "$stage/usr/share/doc/ipp-duplexd-fork/copyright" <<EOF
Format: https://www.debian.org/doc/packaging-manuals/copyright-format/1.0/
Upstream-Name: ipp-duplexd-fork

Files: *
License: MIT
$(sed 's/^/ /;s/^ $/ ./' LICENSE)
EOF

gzip -9n <<EOF > "$stage/usr/share/doc/ipp-duplexd-fork/changelog.gz"
ipp-duplexd-fork ($ver) unstable; urgency=medium

  * Initial release.

 -- Sami Kalliomäki <sami@kalliomaki.me>  $(date -R)
EOF

mkdir -p "$stage/DEBIAN"
cat > "$stage/DEBIAN/control" <<EOF
Package: ipp-duplexd-fork
Version: $ver
Section: net
Priority: optional
Architecture: $arch
Depends: qpdf, libc6, libgtk-4-1, libadwaita-1-0 (>= 1.5)
Recommends: ipp-usb
Suggests: cups-client
Installed-Size: $(du -sk "$stage" --exclude=DEBIAN | cut -f1)
Maintainer: Sami Kalliomäki <sami@kalliomaki.me>
Description: Manual duplex printing via a loopback virtual IPP printer
 ipp-duplexd-fork is a virtual IPP printer listening on 127.0.0.1 that turns a
 simplex printer into a manual duplex printer: it prints the odd pages
 on the real printer, pauses (media-needed) so the user can flip the
 stack, then prints the even pages in reverse order on the backs.
 .
 Register it in CUPS as a driverless queue:
 lpadmin -p NAME -E -v ipp://127.0.0.1:6632/ipp/print -m everywhere
EOF

out="packaging/ipp-duplexd-fork_${ver}_${arch}.deb"
dpkg-deb --build --root-owner-group "$stage" "$out"
echo
echo "install with: sudo apt install ./$out"
