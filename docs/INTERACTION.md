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
| Print the backs | `GET /flip` | The printer row's "print the backs" button → toast | **no** | only offered while that printer has a job waiting |
| Add a virtual printer | a new `[[printer]]` in `config.toml` | `+` → the queues CUPS reports; pick one and it is added; name/address/URI live under **Advanced** | **no** | shared validator; unique name and address; never written over an unparseable file |
| Remove a virtual printer | delete the `[[printer]]` | Row action with a destructive confirmation | **no** | confirmation dialog, cancel is the close response |
| Choose the language | `--lang`, `IPP_DUPLEXD_FORK_LANG`, `LC_*` | Settings → language row; applied to the window at once, to the daemon on restart | config | list comes from the catalogs |
| Printer address, port, settle time | `--printer`, `--listen`, `--settle`, `config.toml` | The printer editor writes `config.toml`; the daemon re-reads it on `POST /reload` | config | shared validator, atomic write, `0600` |
| Restart the service | `systemctl --user restart ipp-duplexd-fork` | Service row button, used when no daemon answers a reload | **no** | — |
| Read the docs, version | `--help`, `--version` | About group | yes | — |

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

What phase A built (0.4.0):

```
┌──────────────────────────────────────────┐
│ HeaderBar: title/subtitle · [+]          │
├──────────────────────────────────────────┤
│ Group "Virtual printers"                 │
│  manual-duplex  127.0.0.1:6632 → ipp://… │
│      [flip] [status] [edit] [remove]     │
│  letter-duplex  127.0.0.1:6633 → ipp://… │
│ Group "Service"     ipp-duplexd-fork …   │
│ Group "Settings"    language, file path  │
│ Group "About"       version, project     │
└──────────────────────────────────────────┘
```

- A `PreferencesPage` of stock `PreferencesGroup`s inside a `ToolbarView` in a
  `ToastOverlay`. GNOME's own settings applications are shaped exactly this
  way, and every row is a widget libadwaita already draws — nothing here is
  hand-painted, so light/dark, accent colour, fonts and RTL come along.
- The rows are rebuilt from the configuration after every change, so the screen
  can never disagree with the file. `AdwToast` carries the result; errors never
  push the layout around.
- The editor is an `AdwAlertDialog` whose `extra_child` is a boxed `ListBox` of
  `AdwEntryRow`s; validation runs on a clone of the configuration, and only a
  valid candidate replaces it. Removal is an `AdwAlertDialog` with the
  destructive response *not* as the default.
- `AdwNavigationSplitView` becomes the right frame once the job list exists
  (phase B below), because then there is more than one page to navigate.
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
- No editing of the daemon's *runtime* state or of its job queue from the window;
  it edits the configuration file (through the same validator the daemon uses)
  and asks the daemon to re-read it. The wizard still owns the legacy
  `ipp-duplexd-fork.conf`; both fronts write files, neither keeps its own copy
  of the truth. No background service of our own.
- No TUI re-implementation: the terminal front is the daemon plus the wizard, and
  it is the reference the window is written against.

## Phases

- **A (done, 0.4.0)** — the manager window as the page above: printers with
  print-the-backs/status/edit/remove, adding, the service row, the language and
  file-path settings, the about group, and every change saved through
  `src/config.rs` then applied with `POST /reload` (service restart when no
  daemon answers).
- **B** — `AdwNavigationSplitView` plus Blueprint templates (`.blp`) once the job
  list makes the window multi-page; that is how GNOME applications keep their UI
  reviewable.
- **C** — the job list (cancel, per-job detail) and the per-printer settings, each
  only after the daemon has the matching operation.
