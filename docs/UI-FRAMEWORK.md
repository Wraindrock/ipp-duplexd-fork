# Which desktop UI toolkit?

`ipp-duplexd-fork` has one small GUI surface today (the "flip the stack"
confirmation) and a larger one planned (a Distroshelf-style manager for the
virtual printer: queues, jobs, settings). The choice of toolkit was made by
shopping around rather than by taking the first thing that worked, and it is
recorded here so the next person can re-check it instead of re-litigating it.

## Decision

**GTK4 + libadwaita**, the toolkit GNOME itself is built on, behind the optional
`gui` Cargo feature. The bindings are the newest official ones and are checked
against the newest official releases before each bump.

| Crate | Version | Checked | Why |
| --- | --- | --- | --- |
| `gtk4` | 0.11.5 | 2026-09-20 | The official Rust bindings of GTK; MSRV 1.83, built for GTK 4.24 API tier |
| `libadwaita` | 0.9.2 (`v1_5`) | 2026-07-07 | The official Rust bindings of GNOME's application framework: `AdwStyleManager` (system light/dark, accent colour, high contrast), `AdwAlertDialog`, `AdwNavigationSplitView` |
| `rust-i18n` | 4 | — | Already used by the daemon and the setup wizard; the window reuses the same catalogs |

Two API tiers exist so that "newest" and "packagas on current distributions"
are both first-class:

| Feature | Toolkit floor | Use |
| --- | --- | --- |
| `gui` (default) | GTK 4.10, libadwaita 1.5 | Every distribution we package for (Fedora 40+, Ubuntu 24.04+, Debian 13+, Arch) |
| `gui-newest` | GTK 4.24, libadwaita 1.10 | The newest official API tier (GNOME 51), for a build host that has it |

CI proves both tiers compile and pass the same tests, so the "newest" claim is a
tested configuration rather than a sentence in a document.

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

### Verified versions (re-checked 2026-10-26)

| Thing | Version | Date | Source |
| --- | --- | --- | --- |
| GNOME release | **51** | 2026-09 | [GNOME release notes](https://release.gnome.org/51/) ("GNOME 51 includes GTK 4.23 and Libadwaita 1.10") |
| GTK (stable) | **4.24.1** | tag 2026-10-01 | [GNOME/gtk tags](https://gitlab.gnome.org/GNOME/gtk/-/tags) |
| libadwaita | **1.10.0** | tag 2026-09-14 | [GNOME/libadwaita tags](https://gitlab.gnome.org/GNOME/libadwaita/-/tags) |
| gtk4 (Rust) | 0.11.5 | 2026-09-20 | [crates.io](https://crates.io/crates/gtk4) |
| libadwaita (Rust) | 0.9.2 | 2026-07-07 | [crates.io](https://crates.io/crates/libadwaita) |

The release-page statement is that GNOME 51 ships GTK 4.23 and libadwaita 1.10;
GTK 4.24 is the stable release cut from that cycle (4.23 is the development
series, whose stable form is 4.24, the same way 4.22 was the stable form of
4.21). libadwaita 1.10 requires GTK 4.23.1 or newer.

### Evidence worth keeping

- libadwaita's `StyleManager` exposes `color-scheme`, `accent-color`,
  `high-contrast` and `connect_color_scheme_notify`; the default scheme is
  "follow the system", which is exactly requirement 2 — no code in this repo
  reads a theme setting.
- `AdwNavigationSplitView` (1.4), `AdwStatusPage` (1.2), `AdwToast` (1.0),
  `AdwToolbarView` (1.4), `AdwAboutDialog` (1.5) and `AdwAlertDialog` (1.5) are
  all part of libadwaita itself, so the HIG patterns need no third-party code.
- Slint's pricing page: open source applications are GPLv3, proprietary use
  needs a paid licence. MIT code that linked it would have to be distributed
  under the GPL.
- iced's theme module documents a built-in `Theme`/`Palette` model; changes to
  the *system* appearance are a separate concern the application must wire up.

## How the requirements are met in the code

- **Light/dark:** `adw::init()` installs an `AdwStyleManager`; the application
  never sets a colour scheme, so the desktop's `color-scheme` wins. For
  testing or screenshots, libadwaita's own override works:
  `ADW_DEBUG_COLOR_SCHEME=prefer-dark|prefer-light`.
- **Translations:** `gui::flip_prompt()` renders `daemon.dialog.text/ok/not_yet`
  through `rust-i18n`; `--lang` and the environment follow the same rules as
  the daemon (`src/l10n.rs`).
- **Look:** `adw::AlertDialog` with a suggested "print the backs" response and
  the "not yet" response as the close/escape action. Started with no arguments
  the same binary opens the window instead: an `adw::PreferencesPage` inside an
  `adw::ToolbarView`, i.e. stock libadwaita widgets, so the desktop's header
  bar, spacing and colours come for free.
- **The window:** the daemon's address, its status page (fetched with the
  window's own `Accept-Language`, so the words match the labels), a refresh
  action, the "print the backs" button, and the version. Nothing is invented:
  the daemon already says what it is doing, the window shows it.
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
