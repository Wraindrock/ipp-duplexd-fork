# Interaction design: one core, three front ends

This document fixes the vocabulary, the operation set and the widget structure
before more of the window is built, so the terminal and the desktop cannot drift
apart. It is the answer to three questions: which parts of the program are
front ends, which operations exist at all, and which of those are read-only
displays versus things the user does.

## Built on the documented GNOME stack, not on a third-party blueprint

The structure below is not borrowed from another application; it is what the
official GNOME documentation prescribes for a GTK4/libadwaita program, and every
widget named here comes from libadwaita itself:

- [GNOME Human Interface Guidelines](https://developer.gnome.org/hig/) — the
  patterns (adaptive navigation, empty/error states, in-app notifications, safe
  defaults for destructive actions).
- [libadwaita documentation](https://gnome.pages.gitlab.gnome.org/libadwaita/doc/)
  — `AdwNavigationSplitView` for navigation, `AdwStatusPage` for empty and error
  states, `AdwToast` (via `AdwToastOverlay`) for results, `AdwAlertDialog` for
  decisions, `AdwPreferencesPage`/`AdwPreferencesGroup` for settings.
- [gtk4-rs book](https://gtk-rs.org/gtk4-rs/stable/latest/book/) — the Rust
  bindings used here.

| Layer | Here |
| --- | --- |
| Behaviour (the source of truth) | the daemon: `src/main.rs`, `src/ipp.rs` |
| Shared vocabulary and helpers | `src/lib.rs` (`ipp`, `l10n`) and `src/gui.rs` (requests, strings, exit codes) |
| Desktop front end | `src/bin/ipp-duplexd-fork-gui.rs` |
| Terminal front ends | the daemon CLI and `src/bin/ipp-duplexd-fork-setup.rs` |
| Declarative templates | phase B below (phase A builds the same tree in code) |

The rule that makes this an architecture rather than a style choice: **a front
end may only do what the core can already do, and the core is the daemon.** The
GUI is a client of the daemon's HTTP interface; it never reads the config file,
never talks to CUPS, and never keeps state of its own.

The toolkits and bindings are always the newest official ones this project can
package against; [docs/UI-FRAMEWORK.md](UI-FRAMEWORK.md) records the versions and
the dates at which they were checked.

## The vocabulary is the contract

Every user-visible sentence lives in `locales/en.yml` and `locales/zh-CN.yml`.
The daemon, the wizard and the window all translate through the same keys, so the
same action is called the same thing in the terminal and on the desktop, in both
languages. A new sentence without a key fails `tests/i18n.rs`; an unused key fails
it too.

## Operation matrix

The daemon is the source of truth; every row has exactly one implementation
there, and the other fronts call it.

| Operation | Daemon (CLI / IPP / HTTP) | Window | Read-only? | Guard |
| --- | --- | --- | --- | --- |
| Show status | `ipp-duplexd-fork` log line, `GET /` | Status page (fetched with the window's `Accept-Language`) | yes | — |
| Refresh | `GET /` | Header bar button, also every 2 s | yes (idempotent) | — |
| Print the backs | `GET /flip` | Suggested "print the backs" button → confirmation dialog | **no** | only offered while a job waits; a job may be printed once |
| Cancel a job | IPP `Cancel-Job` | Row action in the job list | **no** | confirmation dialog, cancel is the default response |
| Choose the language | `--lang`, `IPP_DUPLEXD_FORK_LANG`, `LC_*` | Settings page (shows the current one; changing it writes a hint, it takes effect on restart) | config | — |
| Printer address, port, settle time | `--printer`, `--listen`, `--settle` | Printer page: shown, with the file the daemon reads | config | — |
| Run the wizard | `ipp-duplexd-fork-setup` | About → "set up a printer": opens a terminal running the wizard | **no** | — |
| Read the docs, version | `--help`, `--version` | About page | yes | — |

Two consequences are deliberate:

- **Nothing is editable in the window that the daemon cannot re-read.** A GUI
  text field that rewrites `~/.config/ipp-duplexd-fork.conf` behind the daemon's
  back would give two sources of truth for the same setting. The window shows the
  value and the file; changing it is the wizard's job (it owns the file).
- **The GUI cannot print a job by itself.** It asks the daemon to release the
  backs (the same `GET /flip` the terminal uses), because the daemon is the one
  holding the job and the real printer connection.

## Display versus interaction

Read-only (text, no affordance beyond selecting and copying):

- daemon address and version, the printer URI, the config file path,
- every field of the status page: job name, user, state, last message,
- page counts and the "waiting for the flip" sentence.

Interactive (a real control, with a consequence):

- **Refresh** — idempotent, no confirmation.
- **Print the backs** — guarded: visible only when the daemon reports a job in
  the stopped-for-flip state; clicking opens a question dialog whose default
  response is "not yet"; only the confirmed response sends `GET /flip`.
- **Cancel job** — guarded the same way, with cancel as the default response.
- **Open the wizard / docs** — no confirmation.
- **Language** — a choice that persists in the config, applied on restart.

Anything else that looks like a button in a mock-up is not one: if the daemon has
no operation for it, it does not get a control until the daemon has one *and a
test that fails without it*.

## Layout

```
┌──────────────────────────────────────────┐
│ HeaderBar: title · state · [refresh]     │
├────────────┬─────────────────────────────┤
│ Navigation │  AdwStatusPage (empty)      │
│  Status    │   or the job rows           │
│  Printer   │                             │
│  Printer   │                             │
│  Settings  │                             │
│  About     │                             │
└────────────┴─────────────────────────────┘
```

- `AdwNavigationSplitView`, not a hand-made sidebar: on narrow windows GNOME
  expects the sidebar to collapse into a navigation page.
- `AdwStatusPage` for "no daemon reached", "no job waiting" and error states:
  icon, title, description, one action. It is the only place an error is drawn
  in full; repeated failures appear as `AdwToast` so they do not push the layout.
- Every destructive action is an `AdwAlertDialog` with the safe response as the
  default, matching the daemon's own dialog contract (exit code 0 means "go",
  anything else means "not now").
- The window follows the desktop's light/dark scheme through libadwaita; there is
  no theme setting, and the AppImage ships no toolkit of its own, so the style is
  whatever GNOME the program meets.

## Terminal and desktop stay in step

1. **Same words**: one catalog key per sentence, shared by both fronts.
2. **Same semantics**: the window's button performs the same HTTP request as the
   documented `curl` line, with the same effect.
3. **Same failures**: a daemon that is not running, a printer that is offline and
   a rejected job produce the daemon's own message, translated, not a new one
   invented in the GUI.
4. **Same exit codes**: the flip dialog's 0/1/2 contract is the daemon's
   fallback contract; the window's "print the backs" button goes through the same
   daemon route, so there is only one definition of "yes".
5. **Parity test**: the operation matrix above is the checklist; when a row gains
   behaviour it gains a catalog key, an integration test and a line in this
   document — in that order.

## Non-goals

- No remote control: the daemon listens on loopback only, and the window talks to
  the same loopback address.
- No bundled theme or custom design language; GNOME's is the design.
- No editing of the daemon's configuration from the window (the wizard owns it),
  and no background service of our own.
- No TUI re-implementation: the terminal front is the daemon plus the wizard, and
  it is the reference the window is written against.

## Phases

- **A (now)** — rebuild the current single page as the split view above, with the
  status page, the refresh action and the toast, in code.
- **B** — move the pages into Blueprint templates (`.blp`) once their content is
  stable, which is how GNOME applications keep their UI reviewable.
- **C** — the job list (cancel, per-job detail) and the settings page, each only
  after the daemon has the matching operation.
