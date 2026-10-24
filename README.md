# ipp-duplexd — manual duplex printing via a virtual IPP printer

A small Rust daemon (one dependency, [rust-i18n][i18n]) that turns any
simplex printer into a manual duplex printer. It is itself an IPP printer, listening on
`127.0.0.1` only. Print to it; it prints the **odd pages** on the real
printer, pauses so you can flip the stack and feed it back in, then prints
the **even pages in reverse order** on the backs.

No PPDs and no CUPS backend involved — it registers as a modern driverless
(IPP Everywhere) queue, the architecture future CUPS 3.x mandates.

```
app --> CUPS queue (driverless) --> ipp-duplexd (127.0.0.1:6632)
                                       |  qpdf split
                                       |--> real printer: odd pages (1,3,5,...)
                                       |    (polls until finished, then stops
                                       |     with reason media-needed)
                                       |<-- you flip the stack, GET /flip
                                       '--> real printer: even pages reversed (...,4,2)
```

## Packages

```sh
./packaging/build-arch.sh   # Arch Linux: packaging/arch/ipp-duplexd-*.pkg.tar.zst  (needs makepkg)
./packaging/build-deb.sh    # Debian/Ubuntu: packaging/ipp-duplexd_*.deb            (needs dpkg-deb)
./packaging/build-rpm.sh    # Fedora/RHEL/openSUSE: packaging/ipp-duplexd-*.rpm      (needs rpm-build)
```

All three install `/usr/bin/ipp-duplexd`, the `ipp-duplexd-setup` helper, a
systemd **user** unit, and docs, and depend on `qpdf`. Translations are
compiled into the binaries, so there are no locale files to install or lose.

The RPM spec is [packaging/rpm/ipp-duplexd.spec](packaging/rpm/ipp-duplexd.spec);
it takes the version from `Cargo.toml` (passed in by the build script) and
installs the docs where Fedora expects them (`/usr/share/doc/ipp-duplexd-<version>`).
It fetches the crates while building, so a submission to a distro builder that
has no network (Fedora Koji, OBS) needs the dependencies vendored first, e.g.
with `rust2rpm`; `build-rpm.sh` also verifies the finished package (both
binaries, the unit, the license, the `qpdf` dependency, and that no locale data
was installed) and deletes it rather than leave a broken artifact.

## Setup

After installing (or `cargo build --release`), run:

```sh
ipp-duplexd-setup
```

It walks through the whole setup interactively: pick the real printer
from your CUPS queues (or enter an `ipp://` URI, which is probed),
optionally tweak the flip dialog / rotation / blank-page options, write
`~/.config/ipp-duplexd.conf`, enable and start the systemd user service
(writing a user unit if the packaged one is absent), and register the
virtual printer as a driverless CUPS queue. Every answer has a sensible
default; `--yes` runs non-interactively, `--no-service` / `--no-queue`
skip steps, and `--printer URI` / `--queue NAME` / `--listen ADDR`
preseed answers. `--lang CODE` runs the wizard in that language and
remembers it (`--lang CODE` is written into the config file, so the
service speaks it too). See `ipp-duplexd-setup --help`.

To do the same by hand instead:

```sh
cp /usr/share/doc/ipp-duplexd/ipp-duplexd.conf.example ~/.config/ipp-duplexd.conf
$EDITOR ~/.config/ipp-duplexd.conf     # set --printer for your real printer
systemctl --user enable --now ipp-duplexd
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
stops with `media-needed` and a desktop dialog opens. Flip the printed
stack, reload it, and confirm the dialog — the backs print. Without a
dialog (or from a script) the same is done with:

```sh
curl http://127.0.0.1:6632/flip
```

`GET http://127.0.0.1:6632/` shows job status.

## Languages

The messages, the flip dialog, the setup wizard, and the status page
come in English and Simplified Chinese; more catalogs are one file
each (see [Adding a language](#adding-a-language)).

Pick the language with `--lang CODE`, or leave it to the environment.
First match wins:

1. `--lang CODE` (daemon and setup wizard)
2. `$IPP_DUPLEXD_LANG`
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

What stays English on purpose:

- **IPP protocol values**: attribute names and keywords
  (`job-state-reasons`, `media-needed`, `sides`, ...), the
  `attributes-natural-language` attribute, and printer identity strings
  that are passed through from the real printer. Clients match on these
  strings; translating them would break the protocol.
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
./target/release/ipp-duplexd --printer ipp://127.0.0.1:631/printers/laser
```

For permanent use, set it up as described in [Setup](#setup) above
(`ipp-duplexd-setup` is built alongside as `target/release/ipp-duplexd-setup`,
and writes the user unit for you if the packaged one is absent).

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

`tests/e2e.rs` is an end-to-end suite (`cargo test`): each test starts
its own ipp-duplexd and a mock IPP printer in place of the real one (on
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
| `--blank WHERE` | `leading` | Blank-page padding of the even pass for odd page counts: `leading`, `trailing`, `none`. Switch if the last page of odd-count documents lands on the wrong sheet. |
| `--auto-continue N` | off | Print the backs N seconds after the odd pass finishes instead of waiting for `/flip` (takes precedence over `--gui`) |
| `--no-gui` | (gui on) | Disable the flip dialog. By default a desktop dialog (zenity, kdialog, or xmessage — first one found) opens when it is time to flip; clicking OK prints the backs. `/flip` keeps working in parallel; a dismissed or failed dialog never cancels the job, and without any dialog tool ipp-duplexd just waits for `/flip`. |
| `--rotate-even 0\|180` | `180` | Rotation of the even pages; set to `0` if the backs come out upside down for your flip direction |
| `--poll N` | `2` | Seconds between job-state polls of the real printer |
| `--lang CODE` | from the environment | Language of the messages and the status page (`en`, `zh-CN`); see [Languages](#languages) |

## Behavior details

- **Printer status is mirrored**: `printer-state`, `printer-state-reasons`
  and `printer-is-accepting-jobs` come from the real printer, so the
  virtual queue shows online/idle/processing/stopped (and supply alerts)
  exactly as the hardware reports them. If the real printer is
  unreachable the queue shows stopped with `offline-report`, but keeps
  accepting jobs — they print once it is back. ipp-duplexd's own duplex flow
  overrides this while waiting for a flip (`media-needed`).
- **Printer properties**: `Get-Printer-Attributes` responses are proxied
  from the real printer (cached 30 s), so the virtual printer advertises
  the real hardware's media sizes, resolutions, color modes, and so on.
  Identity, state, document formats, and `sides-supported` are overridden —
  manual duplex is what this printer adds.
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

[i18n]: https://github.com/longbridge/rust-i18n
