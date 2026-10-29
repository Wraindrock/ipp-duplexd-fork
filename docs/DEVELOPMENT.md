# Developing ipp-duplexd-fork

The map of the documentation is [README](README.md); the list of things that
move together with a change is [CHANGE-MAP](CHANGE-MAP.md); the language
catalogs are described in [I18N](I18N.md).

Everything needed to continue this project is in the repository and in GitHub
(the code, the tests, the catalogs, the packaging scripts, the CI workflows and
the release notes attached to every tag). This file records what is *not*
obvious from those files: how to get a toolchain on a machine that has no
compiler and no root, what the checks are, and how a release is cut.

Nothing here needs access to a previous conversation. If the checkout is lost,
clone the repository again and start at "Build requirements".

## Layout

| Path | What lives there |
| --- | --- |
| `src/main.rs` | the daemon: HTTP+IPP server, job state machine, worker that runs qpdf |
| `src/ipp.rs` | IPP message codec and client (no CLI, no GTK) |
| `src/config.rs` | `~/.config/ipp-duplexd-fork/config.toml`: one `[[printer]]` per virtual printer |
| `src/l10n.rs` | language resolution, `t!` helpers, RFC 8011 language spelling |
| `src/gui.rs` | GTK-free parts of the desktop front end (argument parsing, labels, helpers) |
| `src/discover.rs` | asks the local CUPS daemon which queues exist (`CUPS-Get-Printers`) |
| `src/queues.rs` | registers/removes a queue with `lpadmin`/`lpstat` |
| `src/service.rs` | `systemctl --user` status and restart for the window |
| `src/bin/ipp-duplexd-fork-setup.rs` | the terminal wizard (`lpadmin -m everywhere`) |
| `src/bin/ipp-duplexd-fork-gui.rs` | the libadwaita window and the flip dialog |
| `locales/{en,zh-CN}.yml` | the catalogs, compiled into the binaries at build time |
| `docs/` | design notes: UI framework decision, interaction/operation tree, and this file |
| `packaging/` | deb/rpm/Arch/AppImage scripts, the RPM spec, the unit, the desktop entry |
| `.github/workflows/` | `ci.yml`, `release.yml`, `dnf-repo.yml`, `copr.yml` |

## Build requirements

The default build needs only `cargo`. The window (`--features gui`) needs GTK4
≥ 4.10 and libadwaita ≥ 1.5 development headers; `--features gui-newest` asks
for GTK ≥ 4.24 / libadwaita ≥ 1.10 instead (built in CI on Fedora rawhide).
The end-to-end tests need `qpdf` on `PATH`.

```console
cargo build --release                       # daemon + wizard, no GTK
cargo build --release --features gui        # adds the window
```

### No root, no compiler (this workstation)

The machine this project was built on is an Arch-based bootc host with no
system compiler, no `sudo`, and `/tmp` on tmpfs that is wiped between sessions.
The working recipe is: install Rust into `/tmp/rust`, extract an Arch package
closure into `/tmp/arch`, and point the build at it.

```console
# 1. Rust (no root): a minimal toolchain in /tmp
curl -sSf https://sh.rustup.rs -o /tmp/rustup-init.sh
RUSTUP_HOME=/tmp/rust/rustup CARGO_HOME=/tmp/rust/cargo \
    sh /tmp/rustup-init.sh -y --no-modify-path --profile minimal --default-toolchain stable
RUSTUP_HOME=/tmp/rust/rustup CARGO_HOME=/tmp/rust/cargo \
    /tmp/rust/cargo/bin/rustup component add rustfmt clippy

# 2. The Arch packages the GUI and the packagers need, extracted into /tmp/arch.
#    Packages now live under {core,extra}/os/x86_64/ on geo.mirror.pkgbuild.com
#    (the old flat /x86_64/ path is a 404). Resolve the dependency closure from
#    the repo databases (core.db/extra.db are tar.gz files of per-package desc
#    blocks) and extract each package with bsdtar; do NOT extract glibc, bash,
#    systemd or pacman, they would shadow the host's.
```

Seeds used: `gcc libisl libmpc qpdf pkgconf gtk4 cairo pango gdk-pixbuf2
graphene libepoxy harfbuzz fribidi pixman freetype2 fontconfig libxkbcommon
wayland glib2 pcre2 libffi libxml2 libjpeg-turbo libtiff libwebp lcms2 libpng
zlib expat bzip2 xorgproto libx11* libxcb libglvnd librsvg libcups`, plus
`shared-mime-info` (needed by gdk-pixbuf; its `.pc` is in
`/usr/share/pkgconfig`), `libadwaita-1` (needs `ministream`), and for
packaging `rpm-tools rpm-sequoia lua nettle dpkg libmd make guile gc`.

```console
# 3. The environment every build and test needs
export RUSTUP_HOME=/tmp/rust/rustup CARGO_HOME=/tmp/rust/cargo
export PATH=/tmp/rust/cargo/bin:/tmp/arch/usr/bin:$PATH
export LD_LIBRARY_PATH=/tmp/arch/usr/lib
export PKG_CONFIG_PATH=/tmp/arch/usr/lib/pkgconfig:/tmp/arch/usr/share/pkgconfig
export PKG_CONFIG_SYSROOT_DIR=/tmp/arch     # the .pc files say prefix=/usr
export CARGO_TARGET_DIR=/tmp/ipd-target     # keeps the checkout clean
```

- Without `RUSTUP_HOME`/`CARGO_HOME` cargo tries to write `~/.cargo` and dies
  with `could not create home directory`.
- `PKG_CONFIG_SYSROOT_DIR` is what makes the extracted `.pc` files resolve into
  `/tmp/arch` instead of the (empty) host `/usr`.
- `rpmbuild` needs `RPM_CONFIGDIR=/tmp/arch/usr/lib/rpm` and rpm-sequoia needs
  `libnettle.so.8`, which Arch's nettle 4 no longer ships; keep nettle 3.10
  (from `archive.archlinux.org`) first in `LD_LIBRARY_PATH`.
- The packaging scripts build into `./target` on purpose, so unset
  `CARGO_TARGET_DIR` before running `packaging/build-*.sh` (otherwise the deb
  script fails with `install: cannot stat 'target/release/…'`).

## The checks CI runs

```console
cargo fmt --check
cargo clippy --all-targets -- -D warnings              # default build stays GTK-free
cargo clippy --all-targets --features gui -- -D warnings
cargo test                                              # unit, e2e and i18n suites
cargo test --features gui
shellcheck packaging/*.sh                               # PKGBUILD is not POSIX, skip it
./packaging/version.sh check                            # tree and tag agree
```

`tests/i18n.rs` is strict: every catalog key must exist in every language with
the same `%{placeholders}`, every key used in `src/` must exist, and a key that
nothing uses fails the build. It scans `src/*.rs` and `src/bin/*.rs` for
literals starting with `daemon.`, `setup.`, `gui.`, `config.` or `common.`.

The window cannot be *seen* on a headless machine; the exit-code contract and
the GTK code paths are exercised under `xvfb-run` in CI (`GUI_TEST=window`,
`=add`, `=accept`, `=decline`).

## Cutting a release

Versions are dates: `vYYYY.MM.DD-N`, where `N` counts the builds cut that day.

```console
./packaging/version.sh next            # prints the next version (honours TZ)
./packaging/version.sh set             # writes Cargo.toml and Cargo.lock
```

1. Add a `%changelog` entry to `packaging/rpm/ipp-duplexd-fork.spec` — rpmbuild
   refuses entries that are not in descending chronological order.
2. Run the checks above, then `./packaging/build-rpm.sh` and, if `make` is
   available, `make -f .copr/Makefile srpm outdir=/tmp/srpm`.
3. Commit, tag `v<version>`, push the branch and the tag. Pushing a tag starts
   `release.yml` (deb, Arch, RPM, AppImage, GitHub release) and
   `dnf-repo.yml` (rebuilds the GitHub Pages dnf repository).
4. Write the release notes to a file and apply them:
   `gh release edit v<version> -R Wraindrock/ipp-duplexd-fork --notes-file …`
   (the auto-generated body is just a commit list; the notes are the changelog).
5. Verify what actually shipped: follow the *hashed* `primary.xml.gz` href from
   `…/fedora/<rel>/x86_64/repodata/repomd.xml`, and download an asset and list
   it (`bsdtar -tf file.rpm` works here; `rpm`/`rpm2cpio` are not installed).
6. Optionally copy the new AppImage over `~/AppImages/ippduplexd_fork.appimage`
   (that directory is outside the checkout; on bootc hosts a `dnf` install is
   transient and is lost at reboot, the AppImage is not).

## Operational gotchas

- **`/tmp` is tmpfs and is wiped.** Everything in the toolchain section has to
  be rebuilt; nothing in the repository depends on it.
- **Never `pkill -f` a pattern that appears in the same command line** — the
  shell matches itself and kills the job.
- **api.github.com is flaky here** (`TLS handshake timeout`,
  `unexpected eof while reading`): retry `gh` and `git push` calls a few times.
  `gh` must be pointed at the fork explicitly (`-R Wraindrock/ipp-duplexd-fork`)
  because the upstream remote wins by default, and `gh run view --log` needs
  `XDG_CACHE_HOME` to a writable directory. Run logs only appear once the whole
  run finishes.
- **This session's shell is a container** (`/run/.containerenv`): its `/usr` is
  not the host's. The host root is visible at `/run/host` when the host is a
  bootc system; `/var/home/ian` is shared.
- **The AppImage deliberately ships no GTK/libadwaita** (it uses the desktop's,
  so the window matches the desktop's style). It cannot be built on a machine
  without the GTK plugin's expectations; CI builds it on ubuntu-24.04.

## What is local-only

Ignored build outputs (`packaging/*.rpm`, `*.deb`, `*.AppImage`,
`packaging/arch/*`), the copy in `~/AppImages`, and the `/tmp` toolchain. None
of them is required to continue: every published artifact is attached to its
GitHub release, and the dnf repository on GitHub Pages can be rebuilt from the
tag by re-running `dnf-repo.yml`.
