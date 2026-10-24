# RPM spec for Fedora / RHEL / openSUSE, built from the working tree by
# packaging/build-rpm.sh — that script drops a source tarball named
# ipp-duplexd-fork.tar.gz into the source directory and passes the version it read
# from Cargo.toml:
#
#   rpmbuild -bb --define '_topdir DIR' --define 'ipp_version 0.1.3' \
#            --define '_sourcedir DIR/SOURCES' packaging/rpm/ipp-duplexd-fork.spec
#
# The tarball name carries no version, so it can never disagree with the
# version that is passed in (the same trick as packaging/build-arch.sh).
#
# COPR builds this fork from an SRPM instead: .copr/Makefile tars a *versioned*
# tarball (ipp-duplexd-fork-<version>.tar.gz) and bakes the version below into a copy
# of this spec, because an SRPM is rebuilt later on a machine that has no
# working tree and no --define to pass. Keep the anchor lines it rewrites (the
# version on the Version: line, the guard below, Source0 and %setup) exactly one
# each; the Makefile asserts that and fails loudly if this spec drifts.
#
# This is a working-tree build: %build fetches the crates from the network, on
# top of the --locked lockfile. A submission to a distribution that builds
# offline (Fedora Koji, OBS) has to vendor the dependencies first, e.g. with
# `rust2rpm` / `%cargo_vendor`; the file list and the flags below are the part
# that stays.

%{!?ipp_version:%{error:pass --define 'ipp_version <version>'; packaging/build-rpm.sh does}}
%{!?_userunitdir:%global _userunitdir %{_prefix}/lib/systemd/user}

# [profile.release] sets strip = true (Cargo.toml), so there is no debug
# information left to split out; without this the build would make an empty
# debuginfo subpackage (and rpmbuild would insist on find-debuginfo existing).
%global debug_package %{nil}

Name:           ipp-duplexd-fork
Version:        %{ipp_version}
Release:        1%{?dist}
Summary:        Manual duplex printing via a loopback virtual IPP printer
License:        MIT
URL:            https://github.com/Wraindrock/ipp-duplexd-fork
Source0:        ipp-duplexd-fork.tar.gz

# the daemon shells out to qpdf, the setup wizard to lpadmin
Requires:       qpdf
Recommends:     zenity
Recommends:     cups-client
BuildRequires:  cargo
BuildRequires:  rust
BuildRequires:  tar
BuildRequires:  gzip

# This package used to be called ipp-duplexd; replace an installed copy of it
# (up to the last release under the old name) instead of leaving two daemons
# and two systemd units behind.
Obsoletes:      ipp-duplexd < 0.2

%description
ipp-duplexd-fork is a virtual IPP printer listening on 127.0.0.1 that turns a
simplex printer into a manual duplex printer: it prints the odd pages on the
real printer, pauses (media-needed) so the user can flip the stack, then
prints the even pages in reverse order on the backs.

Register it in CUPS as a driverless queue:

    lpadmin -p NAME -E -v ipp://127.0.0.1:6632/ipp/print -m everywhere

The package ships a systemd user unit; enable it per user with
"systemctl --user enable --now ipp-duplexd-fork" after writing
~/.config/ipp-duplexd-fork.conf (see the shipped example).

Messages, the setup wizard and the status page come in English and
Simplified Chinese; the catalogs are compiled into the binaries, so this
package installs no locale data.

%prep
%setup -q -n ipp-duplexd-fork

%build
# Build into the build tree, not wherever the caller's CARGO_TARGET_DIR points
# (the same as packaging/arch/PKGBUILD). No vendored crates here: this fetches
# from crates.io, pinned by Cargo.lock.
export CARGO_TARGET_DIR=target
cargo build --release --locked

%install
export CARGO_TARGET_DIR=target
install -Dpm755 target/release/ipp-duplexd-fork %{buildroot}%{_bindir}/ipp-duplexd-fork
install -Dpm755 target/release/ipp-duplexd-fork-setup %{buildroot}%{_bindir}/ipp-duplexd-fork-setup
install -Dpm644 packaging/ipp-duplexd-fork.service %{buildroot}%{_userunitdir}/ipp-duplexd-fork.service

# tests/ and %check are left out on purpose: the end-to-end suite needs qpdf
# and a mock printer, which belongs in CI (packaging/*.sh run it), not in
# every package build.

%files
%license LICENSE
%doc README.md packaging/ipp-duplexd-fork.conf.example
%{_bindir}/ipp-duplexd-fork
%{_bindir}/ipp-duplexd-fork-setup
%{_userunitdir}/ipp-duplexd-fork.service

%changelog
* Sat Oct 24 2026 Wraindrock <Wraindrock@users.noreply.github.com> - 0.2.0-1
- Renamed the package, both binaries, the systemd unit and the configuration
  file to ipp-duplexd-fork, and added Obsoletes: ipp-duplexd so dnf replaces
  the old package instead of installing a second copy.
- Published an installable dnf repository on GitHub Pages, rebuilt on every
  release.
* Sat Oct 24 2026 Sami Kalliomäki <sami@kalliomaki.me> - 0.1.3-1
- Initial RPM packaging.
- English and Simplified Chinese messages (rust-i18n), compiled into the
  binaries.
