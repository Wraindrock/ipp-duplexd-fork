# Which desktop UI toolkit?

`ipp-duplexd-fork` has one small GUI surface today (the "flip the stack"
confirmation) and a larger one planned (a Distroshelf-style manager for the
virtual printer: queues, jobs, settings). The choice of toolkit was made by
shopping around rather than by taking the first thing that worked, and it is
recorded here so the next person can re-check it instead of re-litigating it.

## Decision

**GTK4 + libadwaita**, behind the optional `gui` Cargo feature:

| Crate | Version | Why |
| --- | --- | --- |
| `gtk4` | 0.10 | Native widgets on every Linux desktop that runs CUPS; MSRV 1.83; requires GTK 4.0 at the sys level |
| `libadwaita` | 0.8 (`v1_5`) | GNOME's GTK4 application framework: platform look, `AdwStyleManager` (system light/dark, accent colour, high contrast), `AdwAlertDialog` |
| `rust-i18n` | 4 | Already used by the daemon and the setup wizard; the dialog reuses the same catalogs |

The dialog is `src/bin/ipp-duplexd-fork-gui.rs`; everything that does not need
GTK (argument parsing, the strings, the exit codes) is in `src/gui.rs` and is
unit-tested without a display.

## Requirements that drove the choice

1. **Native look.** This is a confirmation a user sees in the middle of a print
   job; it should look like every other dialog on their desktop.
2. **Follow the desktop's light/dark scheme automatically** — no in-app theme
   picker, no "dark mode" setting to maintain. Bonus points for accent colour
   and high contrast, which are part of the same integration.
3. **Translations.** English and Simplified Chinese exist today, more may come.
   The strings already live in `locales/*.yml` and are checked by
   `tests/i18n.rs`; the toolkit must not force a second translation system.
4. **Accessible.** A print dialog is a system dialog: it must show up in the
   AT-SPI tree for screen readers and key-only users.
5. **Cheap to package.** `dnf`, `apt` and `pacman` already ship GTK4 and
   libadwaita; no web runtime, Node.js, or bundled rendering stack.
6. **Licence-compatible.** The project is MIT. GTK/libadwaita are LGPL-2.1+ and
   are linked dynamically, which is fine.
7. **Room to grow.** The same binary has to become the manager window later.

## The options that were compared

Scores are for *this* project (a Linux-only, system-integrated print utility),
not for GUIs in general.

| Toolkit | Native look | Auto light/dark | i18n story | A11y | Packaging | Licence | Verdict |
| --- | --- | --- | --- | --- | --- | --- | --- |
| **GTK4 + libadwaita** | yes, and the platform "Adwaita" widgets | **yes, `AdwStyleManager` follows the system `color-scheme`, plus accent colour and high contrast** | `rust-i18n` (already in tree) or `gettext-rs` | AT-SPI | `gtk4`, `libadwaita` in all three distros | MIT crate + LGPL libs (dynamic) | **chosen** |
| Relm4 | yes (it *is* GTK4 underneath) | yes, same `AdwStyleManager` | `relm4` i18n support, gettext-based | AT-SPI | same as GTK4 | MIT/Apache | Viable, but it is an MVU layer on top of gtk4-rs: extra concepts for a one-button dialog. Can be adopted later without changing the stack. |
| Iced | no — draws its own widgets | no; a `Theme` value the app owns, system-appearance plumbing is still open upstream | manual | limited | pure Rust, but needs a GPU/font stack | MIT | Wrong shape: custom look, own renderer, extra font handling. |
| Slint | no — custom look from `.slint` files | yes for the *Slint* palette, not the desktop's | built-in `@tr` | limited | needs its own runtime | **GPLv3 or commercial** | Rejected: linking it would relicense the GUI binary, and it does not look native. |
| egui | no | manual | manual | limited | pure Rust | MIT/Apache | Tool/overlay aesthetic; not for a system dialog. |
| Tauri 2 / Dioxus | no — a webview | via CSS `prefers-color-scheme` | web stack | depends | needs WebKitGTK + an IPC layer | MIT/Apache | Two languages and a web runtime for one dialog; overkill. |
| Xilem / Vizia / Floem | no | varies | manual | mostly missing | early | MIT/Apache | Not mature enough for something users rely on mid-print. |

### Evidence worth keeping

- **Distroshelf**, the manager this project is modelled on, is `gtk4 0.11` +
  `libadwaita 0.9` + `gettext-rs` (its own `Cargo.toml`). The toolkit choice
  above puts the future manager on the same footing.
- libadwaita's `StyleManager` exposes `color-scheme`, `accent-color`,
  `high-contrast` and `connect_color_scheme_notify`; the default scheme is
  "follow the system", which is exactly requirement 2 — no code in this repo
  reads a theme setting.
- Slint's pricing page: open source applications are GPLv3, proprietary use
  needs a paid licence. MIT code that linked it would have to be distributed
  under the GPL.
- iced's theme module documents a built-in `Theme`/`Palette` model; changes to
  the *system* appearance are a separate concern the application must wire up.

### Why not gtk4 0.11 + libadwaita 0.9 (Distroshelf's pair)

Those versions want a newer Rust (1.92) and a newer GTK/libadwaita than the
distributions we package for. `gtk4 0.10` + `libadwaita 0.8`/`v1_5` still gives
`AdwAlertDialog` and `AdwStyleManager`, and is satisfied by Fedora 40+, Ubuntu
24.04+ and Debian 13+. Bumping later is a `Cargo.toml` change plus the same
tests.

## How the requirements are met in the code

- **Light/dark:** `adw::init()` installs an `AdwStyleManager`; the application
  never sets a colour scheme, so the desktop's `color-scheme` wins. For
  testing or screenshots, libadwaita's own override works:
  `ADW_DEBUG_COLOR_SCHEME=prefer-dark|prefer-light`.
- **Translations:** `gui::flip_prompt()` renders `daemon.dialog.text/ok/not_yet`
  through `rust-i18n`; `--lang` and the environment follow the same rules as
  the daemon (`src/l10n.rs`).
- **Look:** `adw::AlertDialog` with a suggested "print the backs" response and
  the "not yet" response as the close/escape action.
- **Exit contract:** 0 = print the backs, 1 = not yet, 2 = the dialog could not
  run (no display). The daemon only continues on 0.

## Non-goals (deliberate)

- Windows and macOS. The daemon talks to CUPS; the desktop story is Linux.
- A custom design language or bundled theme; we inherit the desktop's.
- Shipping fonts (including CJK glyphs); the desktop provides them.
- A web-based UI.

## Escape hatches and known limits

- `--features gui` off: the daemon and the setup wizard build with no GTK
  dependency at all.
- The daemon falls back to `zenity`, `kdialog`, `xmessage`, or plain
  `GET /flip` when the GTK window cannot run — including when it dies before
  opening a window (exit code 2, or killed by a signal). The job is never
  cancelled by a broken dialog; `daemon.log.dialog_failed` says what happened.
- Headless servers: no dialog is expected; use `--no-gui` and `GET /flip`.
- The GTK4 and libadwaita runtime libraries must be installed; the packages
  declare them (`gtk4`, `libadwaita`), and the unsuffixed `ipp-duplexd-fork`
  package keeps working without a desktop since the dialog is only spawned on
  demand.
- The only part of the dialog that cannot be tested on a machine without a
  display is the click path itself; CI runs it under `xvfb-run` with the
  `IPP_DUPLEXD_FORK_GUI_TEST` hook, and the no-display fallback is tested
  headlessly here.
