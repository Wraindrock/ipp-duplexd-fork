# Change map

What else moves when you move something. Each section is one kind of change,
the places it lands, and the command that tells you whether you got them all.
The lists are the ones that have actually been forgotten at least once; they are
not exhaustive by design — the tests are.

Run before every push (or let CI do exactly this):

```sh
cargo fmt && cargo clippy --all-targets --features gui -- -D warnings && cargo test --features gui
```

## A new or changed per-printer option

The option lives in the configuration file first; everything else reads it.

| Layer | File | What to change |
| --- | --- | --- |
| configuration | `src/config.rs` | the field, a `#[serde(default = "…")]` default, `validate()` bounds if any, and the `a_minimal_entry_fills_in_the_defaults` test |
| daemon | `src/main.rs` | the `Settings` field, the `--flag` it grew up with, and the place `flip`/split logic reads it |
| window | `src/gui.rs` | a `WindowLabels` field for the label and the hint |
| | `src/bin/ipp-duplexd-fork-gui.rs` | a row in the editor's *printing behaviour* group, prefilled from `current`, written back into `printer` on save |
| terminal | `src/bin/ipp-duplexd-fork-setup.rs` | the same question in `add_printer` **and** `edit_printer` |
| strings | `locales/{en,zh-CN}.yml` | label + hint, both catalogs |
| user guide | `README.md` | the options table and the `config.toml` example |
| | `README.zh-CN.md` | the same, in the quick start, if it is user-visible |

Check: `cargo test --features gui` (config round-trip and validation), plus a
manual save/edit in the window and in the terminal menu.

## A new window string

| Step | Where |
| --- | --- |
| the value | `locales/en.yml`, then the same key in `locales/zh-CN.yml` |
| the label | `WindowLabels` in `src/gui.rs` (`text("gui.window.…")`) |
| the widget | `src/bin/ipp-duplexd-fork-gui.rs`, reading `labels.…` |
| stale embed locally | `touch src/lib.rs` — rust-i18n embeds the catalogs at build time |

`no_key_is_unused` fails on a catalog key nothing uses, and
`every_key_used_in_the_source_exists` fails on a string with no catalog entry.
See [I18N.md](I18N.md).

## The flip dialog

The daemon spawns it; the window only draws it when asked.

| Piece | Where |
| --- | --- |
| the text | `gui::flip_text()` in `src/gui.rs` (prompt + the instance's own note) |
| the candidates | `spawn_flip_dialog` in `src/main.rs`: this fork's GTK4 dialog, then `zenity`, `kdialog`, `xmessage` |
| the contract | exit code 0 = "back printed, go ahead"; anything else = "not yet" — `tests/gui.rs` and `tests/e2e.rs` hold it |
| the note | `Printer.note` → `Settings.note` → `flip_text`; it appears in every candidate because they all take the same string |

Check: `cargo test --features gui`, and one real job through the window.

## Printer driver, ipp-usb and hplip

The area where a wrong assumption costs an afternoon. The rules live in
`src/usb.rs` (is ipp-usb installed, is it serving, which port, is the device
attached), `src/hplip.rs` (is this queue hplip's, what is the conflict, which
driver is in use) and `src/queues.rs` (`lpadmin` calls, backups, switch and
restore).

| Touching | Also look at |
| --- | --- |
| `usb::serving_port()` | the port range `PORT..=PORT_END`; the window, the terminal menu and `queues::switch_to_ipp_usb` all take the port from it |
| `hplip::driver()` | the row label in the window and the list in the terminal front end |
| `hplip::conflict_for()` / `stuck()` | the row's hint, and `diagnostics::text()` |
| the switch button | one button, both directions — it reads the driver and moves the queue to the other one; do not add a second control |
| `queues::repoint()` | the start-up port correction in both front ends |

Check: the machine in front of you may not be the machine with the printer —
USB passthrough moves the device between the host and a VM, so "no port answers"
is not the same as "ipp-usb is broken". `usb::attached()` exists for that.

## Diagnostics

`diagnostics::text()` in `src/diagnostics.rs` is the single report both front
ends render; the window shows it in a window, the terminal prints it. Anything
you add to it appears in both by construction — do not build a second one.

## Service and the two images

The unit runs `~/.local/share/ipp-duplexd-fork/ipp-duplexd-fork.AppImage`; the
desktop shortcut updates a different file. `service::stale_image()` compares
them and both front ends offer to refresh. The refresh copies beside the target
and renames over it — writing into a file the unit is executing fails with
`ETXTBSY`.

| Piece | Where |
| --- | --- |
| install / uninstall / restart | `src/service.rs`, wrapped by `gui.rs` rows and the terminal menu |
| the unit text | `service::unit_text()`; the packaged unit wins over a written one |
| the stale check | `service::stale_image()`, `service::refresh_image()` |

## Packaging and versions

- `packaging/version.sh set <version>` — never hand-edit the tag; the version
  is `YY.MM.<feature>[-<fix>]`, and `next` / `fix` compute the next one of each
  (see the README's *Version numbers*).
- `packaging/release.sh` — fmt, clippy, shellcheck, build, test, commit
  `Release vX`, tag, push. It refuses a dirty tree.
- A tag triggers `.github/workflows/release.yml` (AppImage, deb, rpm, Arch
  package) and `dnf-repo.yml`. The dnf job can start before the release assets
  exist and fail; it succeeds on the next run — check the Release workflow, not
  the dnf one, to know whether a release landed.
- The three package recipes reference the binary names; renaming one touches
  `packaging/*.spec`, `packaging/PKGBUILD`, `packaging/AppImage.sh`, the
  `.desktop` file and the README install sections.

## Documentation itself

`README.md` and `README.zh-CN.md` are the user guide in two languages and are
kept in step; everything else lives in `docs/` (see [README](README.md) for the
map). A behaviour change updates the README's option/behaviour tables *and* the
entry in [ISSUES.md](ISSUES.md) that explains why it changed.
