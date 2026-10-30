# ipp-duplexd-fork — manual duplex printing via a virtual IPP printer

**English** · [简体中文](README.zh-CN.md)

> This is a fork of [ipp-duplexd][upstream], adding i18n and a GUI manager.
> On Fedora, install it with `dnf` from the [GitHub-hosted repository][repo];
> everywhere else, build a package locally with the scripts in
> [Packages](#packages).

A small Rust daemon (one dependency, [rust-i18n][i18n], plus an optional
GTK4 + [libadwaita][adw] front end) that turns any
simplex printer into a manual duplex printer. It is itself an IPP printer, listening on
`127.0.0.1` only. Print to it; it prints the **odd pages** on the real
printer, pauses so you can flip the stack and feed it back in, then prints
the **even pages in reverse order** on the backs.

No PPDs and no CUPS backend involved — it registers as a modern driverless
(IPP Everywhere) queue, the architecture future CUPS 3.x mandates.

```
app --> CUPS queue (driverless) --> ipp-duplexd-fork (127.0.0.1:6632)
                                       |  qpdf split
                                       |--> real printer: odd pages (1,3,5,...)
                                       |    (polls until finished, then stops
                                       |     with reason media-needed)
                                       |<-- you flip the stack, GET /flip
                                       '--> real printer: even pages reversed (...,4,2)
```

## Documentation

The user guide is this file. Everything else is in [docs/](docs/README.md):

| Read | For |
| --- | --- |
| [docs/OPERATIONS.md](docs/OPERATIONS.md) | every operation, and which front end owns it |
| [docs/INTERACTION.md](docs/INTERACTION.md) | one core, three front ends — the design |
| [docs/I18N.md](docs/I18N.md) | the language catalogs, and what the tests enforce |
| [docs/CHANGE-MAP.md](docs/CHANGE-MAP.md) | what else has to change when you change something |
| [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md) | toolchain, checks, release |
| [docs/UI-FRAMEWORK.md](docs/UI-FRAMEWORK.md) | why GTK4 + libadwaita |
| [docs/ISSUES.md](docs/ISSUES.md) | the log of what was reported and fixed |

## Packages

```sh
./packaging/build-arch.sh   # Arch Linux: packaging/arch/ipp-duplexd-fork-*.pkg.tar.zst  (needs makepkg)
./packaging/build-deb.sh    # Debian/Ubuntu: packaging/ipp-duplexd-fork_*.deb            (needs dpkg-deb)
./packaging/build-rpm.sh    # Fedora/RHEL/openSUSE: packaging/ipp-duplexd-fork-*.rpm      (needs rpm-build)
./packaging/build-appimage.sh # any distro: packaging/ipp-duplexd-fork-*.AppImage (for testing)
```

The three packages install `/usr/bin/ipp-duplexd-fork`, the
`ipp-duplexd-fork-setup` helper, a systemd **user** unit, and docs, and depend on
`qpdf` and the desktop's GTK4 and libadwaita; the AppImage is a test build for
trying it without installing (see below). A **USB** printer needs one more
piece — [`ipp-usb`][ipp-usb], which the packages recommend rather than require;
see [USB printers](#usb-printers).
Translations are compiled into the binaries, so there are no locale files to
install or lose.

The RPM spec is [packaging/rpm/ipp-duplexd-fork.spec](packaging/rpm/ipp-duplexd-fork.spec);
it takes the version from `Cargo.toml` (passed in by the build script) and
installs the docs where Fedora expects them — `/usr/share/doc/ipp-duplexd-fork` on
newer rpmbuilds, `/usr/share/doc/ipp-duplexd-fork-<version>` on older ones.
It fetches the crates while building, so a submission to a distro builder that
has no network (Fedora Koji, OBS) needs the dependencies vendored first, e.g.
with `rust2rpm`; `build-rpm.sh` also verifies the finished package (both
binaries, the unit, the license, the `qpdf` dependency, and that no locale data
was installed) and deletes it rather than leave a broken artifact.

## Version numbers

A version says what a build contains, not when it was typed:

```
26 . 10 . 3 - 2
|    |    |   `-- fix counter: the 2nd fix (or change) for feature 3 and the ones before it
|    |    `------ feature counter: starts at 1 each month, one step per feature update
|    `----------- month
`---------------- year
```

So `26.10.3` is the third feature update in October 2026, `26.10.3-1` the first
fix on top of it (and on top of everything before it), and `26.11.1` the first
feature update in November. Fixes keep the train and the feature they belong
to, so they sort exactly where they belong: after `26.10.3`, before `26.10.4`.
The release tag is `v26.10.3-1` (or `v26.10.3` for a feature release). Cargo
has no room for the fix counter in the number itself, so `Cargo.toml` puts it
in the build metadata — `26.10.3` for the feature, `26.10.3+1` after one fix —
and `packaging/version.sh as-tag` converts one spelling into the other.

`packaging/version.sh next` prints the next feature release (it counts the
tags of the current month; the month comes from the local date, so set `TZ`
when cutting from another timezone, e.g.
`TZ=Asia/Shanghai ./packaging/version.sh next`), `packaging/version.sh fix`
prints the next fix release on top of what `Cargo.toml` currently says,
`packaging/version.sh set [version|fix]` writes it to `Cargo.toml` and
`Cargo.lock`, and `packaging/version.sh check [tag]` proves that a tree and a
tag agree — CI and the release workflow both run it, so a mismatch fails
before anything is published. `packaging/release.sh` does a whole cut:
version, checks, commit, tag, push — `./packaging/release.sh` for a feature
release, `./packaging/release.sh fix` for a fix.

Each package format spells the same number its own way: RPM splits `26.10.3-1`
into `Version: 26.10.3` and `Release: 1` (no dashes allowed in `Version`), Arch
into `pkgver=26.10.3` and `pkgrel=1`, and Debian takes `26.10.3-1` unchanged as
upstream-version/revision.
## Installing on Fedora with dnf

The fork publishes its RPMs as a **dnf repository on GitHub Pages**, rebuilt on
every release. No accounts, no downloading files by hand:

```sh
sudo dnf config-manager addrepo --from-repofile=https://wraindrock.github.io/ipp-duplexd-fork/ipp-duplexd-fork.repo
# dnf4 (Fedora 40 and older): sudo dnf config-manager --add-repo https://wraindrock.github.io/ipp-duplexd-fork/ipp-duplexd-fork.repo
sudo dnf install ipp-duplexd-fork
sudo dnf upgrade            # later: picks up new builds
```

## Trying it without installing (AppImage)

For a quick look without installing anything, build a single-file image — it
carries the daemon, the terminal front end and the GTK4 application. It deliberately does
**not** bundle GTK4 or libadwaita: the window uses your desktop's copies, so it
always matches your current GNOME (or other desktop) style instead of freezing
whatever toolkit the image happened to be built against. Your machine therefore
needs `gtk4` (>= 4.10) and `libadwaita` (>= 1.5), plus `qpdf` for printing:

```sh
./packaging/build-appimage.sh
chmod +x packaging/ipp-duplexd-fork-*.AppImage
./packaging/ipp-duplexd-fork-*.AppImage                 # the window
./packaging/ipp-duplexd-fork-*.AppImage daemon --printer ipp://192.168.1.50/ipp/print
./packaging/ipp-duplexd-fork-*.AppImage setup           # the terminal menu
```

Running it with no arguments opens the window (do not use `sudo`: it has to
run as your desktop user to see your display); `daemon` and `setup` reach the
other two programs. If your distribution does not have FUSE (some minimal or
container setups do not), prefix any of these with `APPIMAGE_EXTRACT_AND_RUN=1`
to unpack it to a temporary directory first. Built images are attached to every
[release](https://github.com/Wraindrock/ipp-duplexd-fork/releases), so you can
also download one instead of building it.

The image carries its update information (`gh-releases-zsync|Wraindrock|
ipp-duplexd-fork|latest|…`) in its `.upd_info` section, and every release
attaches the matching `.zsync` beside the AppImage, so an AppImage manager that
reads it — Gear Lever's **Check for updates**, `appimageupdatetool` — can
replace the file in place instead of downloading a new one by hand. Building
for a different repository only needs
`APPIMAGE_UPDATE_INFORMATION='gh-releases-zsync|owner|repo|latest|…'`.

### Atomic/immutable systems (Bluefin, Bazzite, Silverblue)

`qpdf` is the one runtime dependency with no user-space alternative in the
package, and on an OSTree machine there is no `dnf install`; layering it with
`rpm-ostree` works but costs a reboot and an image change. The shorter way is
Homebrew, which Bluefin and Bazzite already ship:

```sh
brew install qpdf          # no root, no reboot, survives OS updates
```

The service is a *user* unit, so the unit sets its own `PATH`
(`~/.local/bin:/home/linuxbrew/.linuxbrew/bin:/usr/local/bin:/usr/bin:/bin`);
the daemon finds a Homebrew `qpdf` even though the desktop session never
sourced `brew shellenv`. If you prefer a plain binary, dropping one in
`~/.local/bin` works just as well, and `rpm-ostree install qpdf` remains a third
option. The window checks for `qpdf` in all three places before it installs
anything, and says which command to run when it is missing.

It is a test build, not a replacement for the packages: it is unsigned, it
does not install anything or register with the desktop, and the terminal front end still
needs the host's `cups-client` because it has to talk to the real CUPS. The
script pins the AppImage tooling to a release tag and verifies its checksum, and
it exercises the finished image (the three binaries are inside, no GTK4 or
libadwaita is bundled, the dialog's `--help` and `--version` work, the daemon
speaks Chinese with `--lang zh-CN`, and under Xvfb both the window opens and the
dialog confirms) and deletes it rather than leaving a broken artifact.
If your desktop is missing `gtk4`/`libadwaita`, the launcher says so instead of
failing with a loader error.

### Upgrading from the package named `ipp-duplexd`

The fork is called `ipp-duplexd-fork`, so it cannot collide with the upstream
project. Earlier builds were published under the upstream name. `dnf upgrade`
alone will not switch names, so install it explicitly once — the package
carries `Obsoletes: ipp-duplexd` and replaces the old one:

```sh
sudo dnf install ipp-duplexd-fork
mv ~/.config/ipp-duplexd.conf ~/.config/ipp-duplexd-fork.conf   # if you made one
systemctl --user disable --now ipp-duplexd
systemctl --user enable  --now ipp-duplexd-fork
sudo rm /etc/yum.repos.d/ipp-duplexd.repo   # the repository file is now ipp-duplexd-fork.repo
```

CUPS queues point at `ipp://127.0.0.1:6632/ipp/print` and keep working; only the
package, the service unit and the config file changed names.

The repository is built in Fedora 43, Fedora 44 and Rawhide containers, and
`$releasever`/`$basearch` in the repository file select the right directory, so
this works on those three. On anything else — RHEL, openSUSE, an older or newer
Fedora — `dnf` fails with "repomd.xml not found"; install the `.rpm` from the
[releases page](https://github.com/Wraindrock/ipp-duplexd-fork/releases)
instead. The repository keeps only the newest build per Fedora release; older
versions stay on that page.

The packages are **not signed**, so the repository file sets `gpgcheck=0`. That
is as much trust as downloading a `.rpm` by hand from the same project — no
more, and if that matters to you, build the package yourself with
`packaging/build-rpm.sh`.

Removing it again:

```sh
sudo dnf remove ipp-duplexd-fork
sudo rm /etc/yum.repos.d/ipp-duplexd-fork.repo
```

`.github/workflows/dnf-repo.yml` does the work: build the RPM, index it with
`createrepo_c`, then **install it from that index in a clean container** and
check the installed files and both languages before anything is published to
the `gh-pages` branch. A broken file list or index therefore fails the release
instead of reaching users.

A COPR project was the other option, and it is not set up: COPR needs a Fedora
account, and it has no GitHub login. The pieces are in the repository
([`.copr/Makefile`](.copr/Makefile) and
[`.github/workflows/copr.yml`](.github/workflows/copr.yml)); with an account,
`copr-cli create ipp-duplexd-fork --chroot fedora-43-x86_64`, store `~/.config/copr`
as the `COPR_CONFIG` secret, and `sudo dnf copr enable wraindrock/ipp-duplexd-fork`
becomes an alternative to the repository above.

## Setup

After installing (or `cargo build --release`), run:

```sh
ipp-duplexd-fork-setup
```

It opens the terminal menu: **virtual printers** (add, edit, remove, print
the backs, look at a status page, switch a queue between hplip and IPP),
**jobs** (list and cancel), **service** (install, uninstall, restart),
**logs and status**, and **settings** — the same tree the window shows, and
it writes the same `~/.config/ipp-duplexd-fork/config.toml`, so the two
front ends cannot drift apart. Every question has a sensible default.

For scripts, `--printer URI` configures one printer without asking
(defaults everywhere else); `--yes` takes every default, `--no-service` /
`--no-queue` skip the service and the CUPS queue, `--listen ADDR` picks the
address, and `--lang CODE` runs in that language and remembers it in the
config file, so the service speaks it too. See
`ipp-duplexd-fork-setup --help`.

The same first run can be done from the desktop window (`ipp-duplexd-fork-gui`
with no arguments), which is the point of the "This computer" row: adding the
first virtual printer installs the systemd user service when the machine has
none — writing the unit, creating the `~/.config/ipp-duplexd-fork.conf` the unit
names, and enabling it for every login — then waits until the daemon answers and
registers the CUPS queue. A machine whose service is installed but stopped gets
the same treatment (install is idempotent, and ends with a restart). The terminal
front end is for machines without a desktop; both write the same
`config.toml` and offer the same operations.

To do the same by hand instead:

```sh
cp /usr/share/doc/ipp-duplexd-fork*/ipp-duplexd-fork.conf.example ~/.config/ipp-duplexd-fork.conf
$EDITOR ~/.config/ipp-duplexd-fork.conf     # set --printer for your real printer
systemctl --user enable --now ipp-duplexd-fork
lpadmin -p laser-duplex -E -v ipp://127.0.0.1:6632/ipp/print -m everywhere
```

The `lpadmin` line registers the virtual printer as a driverless CUPS
queue; pick any queue name, then print with `lp -d laser-duplex file.pdf`.
Best: point `--printer` at a **local CUPS queue** for the real printer
(`ipp://127.0.0.1:631/printers/laser`), so CUPS keeps doing any driver
work and TLS printers work too.

## Printing

Print to the virtual queue like to any other printer (or `lp -d
laser-duplex file.pdf`). The odd pages come out first; then the job
stops with `media-needed` and a desktop dialog opens (the GTK4 window
shipped with the packages; see [Desktop dialog](#desktop-dialog-gtk4)
for the fallbacks). Flip the printed stack, reload it, and confirm the
dialog — the backs print. Without a dialog (or from a script) the same is
done with:

```sh
curl http://127.0.0.1:6632/flip
```

`GET http://127.0.0.1:6632/` shows job status.

## USB printers

Everything here is IPP: the daemon accepts IPP jobs on a loopback port and
forwards them to another IPP endpoint (`real`). A printer on the end of a USB
cable does not speak IPP by itself, so it needs something that turns it into an
IPP endpoint — that is [`ipp-usb`][ipp-usb]. It talks to the USB device and
exposes it as a normal IPP printer on `ipp://localhost:60000/ipp/print` (60000
is its default and the port it is usually found on).

With `ipp-usb` running, a USB printer is configured like any other:

```sh
ipp-duplexd-fork-setup            # lists the 127.0.0.1:60000 printer it detects
# or by hand:
ipp-duplexd-fork --printer ipp://localhost:60000/ipp/print --listen 127.0.0.1:6632
```

Install it with your package manager — `sudo dnf install ipp-usb` (Fedora),
`sudo apt install ipp-usb` (Debian/Ubuntu), `sudo pacman -S ipp-usb` (Arch,
where it is in the `extra` repository). The packages list it as a
*recommendation*, not a hard dependency: network printers and printers already
behind CUPS never need it.

**A queue that goes through hplip will not print.** HP's `hplip` opens the same
USB device through its own backend; while `ipp-usb` holds it, hplip cannot, and
the queue sits there retrying without ever saying why. You do **not** have to
uninstall hplip — other software may need it — but the CUPS queue for this
printer must stop pointing at `hp:/usb/…`. One command fixes it:

```sh
lpadmin -p QUEUE -E -v ipp://localhost:60000/ipp/print -m everywhere
```

The manager window offers exactly that when it notices such a queue, keeps the
old device URI and PPD, and can switch it back.

If `real` points at a loopback `60000` endpoint and `ipp-usb` is not installed,
`GET /` on the virtual printer says so, instead of leaving jobs to fail with a
timeout that looks like a broken printer.

## Desktop app (GTK4 + libadwaita)

`ipp-duplexd-fork-gui` is the desktop face of the program, built with
[libadwaita][adw] (GNOME's GTK4 application framework) and shipped in the Arch,
Debian, RPM packages and the AppImage. Because it is a regular libadwaita
application it follows the desktop's **light/dark colour scheme
automatically** — there is no theme setting in ipp-duplexd-fork, and accent
colour and high contrast come along for free. (For a one-off override,
libadwaita honours `ADW_DEBUG_COLOR_SCHEME=prefer-dark` or `prefer-light`.)

The packages install an application-menu entry (**ipp-duplexd fork**, with the
icon below) so the window can be started by clicking it; the XML application id
is `io.github.wraindrock.ipp_duplexd_fork`, which is also the name of the
installed `.desktop` file, so the desktop groups the windows under that entry.
The AppImage carries the same two files at the image root.

Start it with no arguments (from your desktop menu, or in a terminal **without
`sudo`**) and it opens the manager window. Everything the command line can do
is there, as one tree:

- **First run** — a machine with no unit (or one that is not running yet) opens
  with a "This computer" row above the printers: **Set up** writes
  `~/.config/systemd/user/ipp-duplexd-fork.service` when the package did not,
  creates the `~/.config/ipp-duplexd-fork.conf` the unit names, has systemd
  `enable --now` it, waits for the daemon, and registers the queues. Saving the
  first printer does the same without a separate click, so a fresh machine can
  be set up from the window alone (the terminal wizard stays for machines with
  no desktop; both write the same files). From an AppImage the image copies
  itself to `~/.local/share/ipp-duplexd-fork/ipp-duplexd-fork.AppImage` first:
  a unit pointing into the temporary mount would start a program that is gone
  the moment the window closes. `qpdf` is checked before anything is installed,
  because the daemon exits immediately without it.
- **Virtual printers** — one row per printer in the
  [configuration file](#configuration-file), showing its listen address and
  the real printer behind it. Each row has four buttons: print the backs of
  the waiting job (the same thing as `GET /flip`), look at that printer's
  status page, edit it, remove it.
- **Adding a printer** — the `+` in the header asks the local CUPS daemon
  (`CUPS-Get-Printers`) which queues it has and lists them: pick one and it is
  added with a unique name (`<queue>-duplex`), the next free loopback address
  and `ipp://127.0.0.1:631/printers/<queue>` as the printer behind it. The name,
  address and URI are still there under **Advanced**, folded away unless
  nothing was detected; that is also how to reach a printer CUPS does not know.
  Adding (or editing) a printer also registers it **in CUPS**, so it shows up
  in your desktop's printer settings (GNOME Settings → Printers) and any
  application can print to it: the window runs
  `lpadmin -p NAME -E -v ipp://ADDR/ipp/print -m everywhere`, the same call the
  setup wizard makes. It needs `cups-client` (for `lpadmin`) and a printer that
  answers; a printer whose daemon is not running is registered the next time
  the window starts, which also repairs printers created by earlier versions.
  Removing a printer removes its CUPS queue too; removing the last one leaves
  an empty configuration, which is allowed — the daemon then serves nothing
  until a printer is added again.
- **Service** — what `systemctl --user` says about
  `ipp-duplexd-fork.service`, a button that restarts it, and — once a unit is
  there — **Uninstall**: the service is stopped and disabled and the unit this
  window wrote is deleted (a packaged unit is only stopped, because the package
  owns it). The printers, their CUPS queues and the configuration file stay, so
  **Set up** brings the same machine back; that is also how a first run can be
  tried on a machine that already has the service.
- **Diagnostics** — **Open** shows one selectable, scrollable block: the unit
  file and its `ExecStart=` line, `systemctl --user status`, the last 80 lines of
  the service journal, and whether each configured port answers. **Copy** puts
  it on the clipboard. The same dialog opens by itself when **Set up** installs
  the service but nothing answers on the port — a toast cannot hold a journal
  and cannot be copied.
- **Settings** — the language of the window (and, after the next restart, of
  the daemon) and the path of the file the window writes.
- **About** — the version and the project page.

Every change is validated first, written atomically to
`~/.config/ipp-duplexd-fork/config.toml` (mode `0600`), and then handed to the
running daemon with `POST /reload`, so adding or removing a printer takes
effect without a restart. When no daemon answers, the window restarts the
user service instead. A file that does not parse is reported as an error and
is never overwritten. Closing the window never touches a running job.

The same binary is what the daemon starts automatically when the odd pages are
done:

```sh
ipp-duplexd-fork-gui --flip "report.pdf [odd pages]" --lang zh-CN
```

The exit code is the answer: **0** prints the backs, **1** leaves the job
waiting, **2** means the dialog could not run (no display) and the daemon
keeps waiting for `GET /flip`. Closing the window or pressing Escape is
"not yet", so a stumbled dialog can never cancel a print job.

If the GUI binary is missing (a build without the feature) or cannot start,
the daemon falls back to `zenity`, then `kdialog`, then `xmessage`, and
finally to plain `GET /flip`.

The GTK4 part is an optional Cargo feature, so the daemon itself still
builds anywhere without GTK4 headers:

```sh
cargo build --release                 # daemon + setup wizard, no GTK4
cargo build --release --features gui  # adds ipp-duplexd-fork-gui
cargo build --release --features gui-newest  # the same, against GTK 4.24/libadwaita 1.10
```

`gui` needs GTK 4.10 and libadwaita 1.5, which every distribution that packages
this program has. `gui-newest` asks the build host for the newest official
tiers (GTK 4.24, libadwaita 1.10, as shipped by GNOME 51) and is built in CI
against Fedora rawhide, so the "newest" claim is tested rather than assumed.
The bindings are `gtk4` 0.11 and `libadwaita` 0.9; [docs/UI-FRAMEWORK.md](docs/UI-FRAMEWORK.md)
records when those versions were verified.

The job list (watch the queue, cancel a job) is the next page for the same
binary. Why GTK4 + libadwaita and not
Relm4, Iced, Slint or a web UI is written down in
[docs/UI-FRAMEWORK.md](docs/UI-FRAMEWORK.md); the vocabulary, the operation set
and which controls are read-only versus guarded, and how the window and the
terminal stay in step, are written down in
[docs/INTERACTION.md](docs/INTERACTION.md) — one core behind every front end,
following the [GNOME Human Interface Guidelines](https://developer.gnome.org/hig/)
and the libadwaita widgets they describe. The window and the dialog share
the [language settings](#languages): `--lang`, `IPP_DUPLEXD_FORK_LANG` and the
usual `LANG`/`LC_*` variables pick the language of the labels, and the window
asks the daemon for its status with the same `Accept-Language`, so both speak
the same language.


## Configuration file

The manager window writes `~/.config/ipp-duplexd-fork/config.toml` (an
`XDG_CONFIG_HOME` is honoured, and `IPP_DUPLEXD_FORK_CONFIG` overrides the
whole path). One daemon process serves every `[[printer]]` in it, each on its
own listen address:

```toml
[settings]
language = "zh-CN"          # optional; defaults to the environment

[[printer]]
name = "manual-duplex"
listen = "127.0.0.1:6632"
real = "ipp://127.0.0.1:631/printers/laser"
blank = "none"              # leading, trailing or none
settle = 10
poll = 2
rotate_even = 0
auto_continue = 0           # 0 = wait for GET /flip
gui = true

[[printer]]
name = "letter-duplex"
listen = "127.0.0.1:6633"
real = "ipp://192.168.1.20/ipp/print"
```

`POST /reload` re-reads the file and starts, stops or replaces listeners to
match it (the window calls it for you, and every answer lists what changed).
Starting the daemon with `--config PATH`, or with no arguments while the file
exists, serves the file; the old single-printer flags and
`~/.config/ipp-duplexd-fork.conf` keep working when there is no file, and the
file wins over flags when both are given. Changing the language in the file
affects the daemon's messages on its next start.


## Languages

The messages, the flip dialog, the setup wizard, and the status page
come in English and Simplified Chinese; more catalogs are one file
each (see [Adding a language](#adding-a-language)).

Pick the language with `--lang CODE`, or leave it to the environment.
First match wins:

1. `--lang CODE` (daemon and setup wizard)
2. `$IPP_DUPLEXD_FORK_LANG`
3. `$LANGUAGE` (colon-separated list)
4. `$LC_ALL`, then `$LC_MESSAGES`, then `$LANG`

Tags follow the usual shapes (`zh_CN.UTF-8`, `zh-CN`, `en_US@euro`); a
locale we have no catalog for falls back to its primary language, so
`zh-TW` and `zh-Hant` get the Simplified Chinese catalog, and anything
untranslated falls back to English. An unknown value on the command
line is an error that lists the known codes.

The status page and `GET /flip` answer in the language of the request's
`Accept-Language` header and say which one they used
(`Content-Language`); IPP `job-state-message` and the log follow the
daemon's own language. The status page is the only thing that is
per-request: the daemon logs once, in its configured language.

The IPP attributes that describe a language follow the configured
language too: `attributes-natural-language` and
`natural-language-configured` carry it, and
`generated-natural-language-supported` has exactly one value, because
the generated text only comes in that one language. RFC 8011
[Section 5.1.9][rfc8011-lang] requires these values to be all lowercase,
so `--lang zh-CN` is announced as `zh-cn`; a mixed-case tag makes CUPS
refuse the printer (`lpadmin -m everywhere` fails its validation), which
is why this is a hard rule rather than a preference.

What stays English on purpose:

- **IPP protocol values**: attribute names and keywords
  (`job-state-reasons`, `media-needed`, `sides`, ...), the
  `attributes-natural-language` value's *form* (always a lowercase
  RFC 8011 tag, never our `zh-CN` spelling), and printer identity
  strings that are passed through from the real printer. Clients match
  on these strings; translating them would break the protocol.
- `anonymous` / `untitled`, used as job owner/name defaults that are
  forwarded to the real printer.
- Diagnostics of malformed IPP requests (the body of a 400 reply is a
  protocol-level hint for whoever built the request).
- The systemd unit's `Description=` (it is a package artifact, not
  output), and the literal `Y/n` / `y/N` answer hints in the wizard
  (answers `y`/`yes`/`n`/`no` — and `是`/`否` — are all accepted).

### Adding a language

1. Copy `locales/en.yml` to `locales/<code>.yml` and translate the
   values, keeping every key and every `%{placeholder}` intact.
2. Add `("<code>", "<native name>")` to `CATALOG` in `src/l10n.rs`.
3. Run `cargo test`. `tests/i18n.rs` fails if a key is missing or has
   different placeholders, if a key is used in the source but absent
   from the catalog, or if a catalog key is unused; there is also a
   test per language that the binaries and the status page actually
   speak it.

## Building from source

Any Rust toolchain works; `qpdf` must be installed. The first build
fetches rust-i18n and its (compile-time only) parsing dependencies, so
`Cargo.lock` is checked in and the packaging scripts build `--locked`. To try it in the foreground, point it at the real
printer's `ipp://` endpoint:

```sh
cargo build --release
./target/release/ipp-duplexd-fork --printer ipp://127.0.0.1:631/printers/laser
```

Building the GTK4 dialog as well needs the GTK4 and libadwaita development
files (`libgtk-4-dev libadwaita-1-dev` on Debian/Ubuntu, `gtk4-devel
libadwaita-devel` on Fedora, `gtk4 libadwaita` on Arch); at runtime the
packages pull in `libgtk-4-1`/`gtk4` and `libadwaita-1-0`/`libadwaita`:

```sh
cargo build --release --features gui
```

For permanent use, set it up as described in [Setup](#setup) above
(`ipp-duplexd-fork-setup` is built alongside as `target/release/ipp-duplexd-fork-setup`,
and writes the user unit for you if the packaged one is absent).

## Development

The toolchain recipe for this workstation, the list of checks, the release
procedure and the operational gotchas are in [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md);
[docs/CHANGE-MAP.md](docs/CHANGE-MAP.md) lists what else has to move when one
piece does. Nothing there is required to *use* the program.

## Formatting

Code is formatted with `rustfmt` (`cargo fmt`). If you use
[jj](https://jj-vcs.dev), `.jjconfig.toml` in the repo root has a `jj fix`
tool that formats every mutable commit. jj does not read tracked config
files on its own, so either pass it per invocation:

```sh
jj --config-file .jjconfig.toml fix
```

or append it to your repo config once after cloning, so plain `jj fix`
works:

```sh
JJ_EDITOR='tee -a' jj config edit --repo < .jjconfig.toml
```

(`jj config edit` opens the repo config in `$JJ_EDITOR`; `tee -a` appends
this file to it instead, leaving any repo config you already had intact.)

## Tests

`tests/i18n.rs` checks the catalogs against each other and against the
sources, and runs both binaries to check that a requested language is
actually used (usage text, setup wizard, status page, `/flip`,
`Content-Language`).

`tests/gui.rs` (and the unit tests in `src/gui.rs`) cover the dialog's
command line and its exit-code contract without a display; with
`cargo test --features gui` the dialog binary is built too, and CI drives
its real widgets under Xvfb.

`tests/e2e.rs` is an end-to-end suite (`cargo test`): each test starts
its own ipp-duplexd-fork and a mock IPP printer in place of the real one (on
kernel-assigned ports, so they run in parallel) and plays CUPS as the
IPP client. Covered: attribute proxying and state mirroring (including
the unreachable-printer case), duplex splitting, blank padding,
rotation, copies, page ranges, one-sided pass-through, and the flip
dialog (via a stub zenity). Set `E2E_KEEP=1` to keep the temp dirs and
daemon logs of a failing run. Needs qpdf:

```sh
cargo test
```

## Options

| Flag | Default | Meaning |
|---|---|---|
| `--printer URI` | (required) | Real printer: `ipp://host[:port]/path` |
| `--listen ADDR:PORT` | `127.0.0.1:6632` | Listen address (warns if non-loopback) |
| `--name NAME` | `manual-duplex` | Printer name to advertise |
| `--blank WHERE` | `none` | Blank-page padding of the even pass for odd page counts: `leading`, `trailing`, `none`. Switch if the last page of odd-count documents lands on the wrong sheet. |
| `--auto-continue N` | off | Print the backs N seconds after the odd pass finishes instead of waiting for `/flip` (takes precedence over `--gui`) |
| `--no-gui` | (gui on) | Disable the flip dialog. By default the GTK4 window shipped with the packages (then zenity, kdialog, or xmessage — first one that runs) opens when it is time to flip; clicking OK prints the backs. `/flip` keeps working in parallel; a dismissed or failed dialog never cancels the job, and without any dialog tool ipp-duplexd-fork just waits for `/flip`. |
| `--rotate-even 0\|180` | `0` | Rotation of the even pages; set to `0` if the backs come out upside down for your flip direction |
| `--poll N` | `2` | Seconds between job-state polls of the real printer |
| `--settle N` | `10` | Seconds to give the real printer to finish and reset after the odd pass. The backs are only sent once it reports `printer-state=idle`; if it stays busy or unreachable for `N` seconds they are sent anyway, and a transient rejection is retried. `0` sends them straight away. |
| `--lang CODE` | from the environment | Language of the messages and the status page (`en`, `zh-CN`); see [Languages](#languages) |
| `--config PATH` | (automatic) | Serve every `[[printer]]` from this file instead of the single printer given by flags; see [Configuration file](#configuration-file) |

## Behavior details

- **Printer status is mirrored**: `printer-state`, `printer-state-reasons`
  and `printer-is-accepting-jobs` come from the real printer, so the
  virtual queue shows online/idle/processing/stopped (and supply alerts)
  exactly as the hardware reports them. If the real printer is
  unreachable the queue shows stopped with `offline-report`, but keeps
  accepting jobs — they print once it is back. ipp-duplexd-fork's own duplex flow
  overrides this while waiting for a flip (`media-needed`).
- **Printer properties**: `Get-Printer-Attributes` responses are proxied
  from the real printer (cached 30 s), so the virtual printer advertises
  the real hardware's media sizes, resolutions, color modes, and so on.
  Identity, state, document formats, and `sides-supported` are overridden —
  manual duplex is what this printer adds.
- **Settling before the backs**: a printer that has just printed the odd
  pages is often still spinning down, or has fallen asleep; sending the
  backs immediately is what made them vanish on USB printers. The daemon
  waits for the real printer to report `printer-state=idle` (at most
  `--settle` seconds) and retries the even job on transport or transient
  server errors.
- **Sides**: jobs with `sides=two-sided-*` (the advertised default) get the
  manual duplex treatment; `sides=one-sided` jobs pass through unchanged.
- **Options**: job attributes (`media`, `print-quality`,
  `printer-resolution`, `print-color-mode`, ...) are forwarded to the real
  printer for both passes. `copies` (replicated so one flip covers all
  copies, collated) and `page-ranges` are applied locally before the split.
  `number-up` is not forwarded — it would change the pagination underneath
  the split.
- **Odd page counts** get a blank page padded into the even pass so the
  sheet pairing stays aligned (see `--blank`).
- **Cancel** works mid-job: the remote pass is canceled best-effort.
- **Flip direction depends on your printer.** Odd-first + reversed evens is
  the standard scheme for face-up output stacks. Do one two-page test print
  to learn the right flip; use `--rotate-even 0` if backs are upside down.
- **`--gui` under systemd**: if the dialog does not appear, the user
  session's display variables are missing from the service environment;
  run `systemctl --user import-environment DISPLAY WAYLAND_DISPLAY` once
  (or add it to your session startup).

## Limitations

- The window manages `config.toml`; a setup that still runs on the legacy
  `~/.config/ipp-duplexd-fork.conf` flags is not imported automatically — add
  the printer once in the window and the file takes over.
- Talks plain `ipp://` to the real printer — no TLS (`ipps://`). For
  TLS-only printers, forward via a local CUPS queue as shown above.
- PDF jobs only (`application/pdf`), which is what CUPS sends driverless
  printers by default.
- One document per job (no multi-document Send-Document sequences).
- Two catalogs so far (English, Simplified Chinese). Everything not in a
  catalog falls back to English; translated *text* only — see
  [Languages](#languages) for what stays English and why.
- The language is per daemon, not per job: `job-state-message` and the
  log use the configured language, only the HTTP status page follows
  `Accept-Language`.

[adw]: https://gitlab.gnome.org/GNOME/libadwaita
[i18n]: https://github.com/longbridge/rust-i18n
[ipp-usb]: https://github.com/OpenPrinting/ipp-usb
[rfc8011-lang]: https://www.rfc-editor.org/rfc/rfc8011.html#section-5.1.9
[upstream]: https://github.com/SamiKalliomaki/ipp-duplexd
[repo]: https://wraindrock.github.io/ipp-duplexd-fork/
