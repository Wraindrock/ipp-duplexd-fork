# Operation tree

Everything a person can do to this program, arranged as one tree, with the
layer that owns each node and the rules that guard it. The desktop window is
the primary front end: any operation that changes something is reachable there.
The terminal front end stays for headless machines and for the installer, and
it calls the same code, so the two cannot drift.

When a change touches one of these nodes, [CHANGE-MAP](CHANGE-MAP.md) says what
else has to move with it.

Vocabulary used below:

- **manager** — the window (`ipp-duplexd-fork-gui` with no arguments).
- **instance** — one virtual printer: one listen address, one real printer, its
  own jobs, its own flip gate. One file describes all of them:
  `~/.config/ipp-duplexd-fork/config.toml`.
- **daemon** — `ipp-duplexd-fork --config …`, one process serving every
  instance in that file.
- **unit** — the systemd user service that runs the daemon
  (`systemctl --user`), if the machine has systemd.

```
ipp-duplexd-fork
├── 1. The service (the daemon process)
│   ├── 1.1 show whether it runs            [read]   manager · `systemctl --user status` · `curl GET /`
│   ├── 1.2 start / restart                 [do]     manager · `systemctl --user restart`
│   ├── 1.3 reload the configuration        [do]     manager (automatic after a save) · `curl POST /reload`
│   └── 1.4 read the log                    [read]   manager (last lines) · `journalctl --user -u …`
├── 2. Instances (virtual printers)
│   ├── 2.1 list them with live state       [read]   manager sidebar · `curl GET /` per port
│   ├── 2.2 add an instance                 [do]     manager "＋" · edit the file · `--config`
│   │   └── fields: name, listen, real printer, page order, settle, poll, rotate, auto-continue, dialog on/off
│   ├── 2.3 edit an instance                [do]     manager (same dialog, prefilled)
│   ├── 2.4 remove an instance              [do]     manager, confirmation names the instance
│   ├── 2.5 show one instance's status      [read]   manager page · `GET /` (its own Accept-Language)
│   └── 2.6 open its printer queue in CUPS  [read]   manager link · `xdg-open`/browser
├── 3. Jobs of one instance
│   ├── 3.1 watch the queue                 [read]   manager page (refreshes every 2 s) · `GET /`
│   ├── 3.2 print the backs                 [do]     manager button → confirmation · `GET /flip`
│   ├── 3.3 cancel a job                    [do]     manager row action → confirmation · IPP `Cancel-Job`
│   └── 3.4 follow one job's messages       [read]   manager page · `job-state-message` over IPP
├── 4. The flip dialog                      (spawned by the daemon, never by the manager)
│   ├── 4.1 confirm "the stack is flipped"  [do]     window button · exit code 0
│   ├── 4.2 postpone                        [do]     Escape/close · exit code 1 · stay waiting
│   └── 4.3 report "no display"             [read]   exit code 2 · daemon falls back
├── 5. Settings
│   ├── 5.1 language of the window          [do]     manager settings · `--lang`, `IPP_DUPLEXD_FORK_LANG`, `LC_*`
│   ├── 5.2 default values for new instances[do]     manager settings (page order, settle, poll, rotate)
│   └── 5.3 where the configuration lives   [read]   manager settings: the path, with a "reveal" action
└── 6. About
    ├── 6.1 version and licence             [read]   manager about page · `--version`
    └── 6.2 links (project, HIG, IPP docs)  [read]   manager about page
```

## The rules that make this a tree and not a list

1. **One owner per operation.** Section 1 belongs to the process manager, 2 and
   5 to the configuration file, 3 to one instance's daemon, 4 to the daemon's
   dialog. The manager is a client everywhere: it never writes a job state and
   never invents a status.
2. **Configuration changes are all-or-nothing.** The manager validates the whole
   file, writes it to a temporary file, renames it over the old one (0600), and
   only then asks the daemon to reload. A rejected edit leaves the running
   daemon untouched.
3. **Every mutation has a guard.** Adding asks for the fields and refuses
   duplicates (same name or same listen address, and a listen address that
   cannot be bound). Removing asks for confirmation and names the instance.
   Printing the backs is offered only while a job waits.
4. **Reads never fail the window.** A stopped daemon, a missing port or a
   missing configuration file is a normal state, shown as such; it is not an
   error dialog.
5. **The tree is the same in both front ends.** The terminal commands in
   brackets are the same operations, not a second implementation.

## Where each node lives in the code

| Node | Code |
| --- | --- |
| 2.2–2.4, 5.1–5.2 | `src/config.rs` (parse, validate, save) and the manager window |
| 1.2–1.4 | `src/service.rs` (systemd user unit detection, start/restart, log tail) |
| 2.5, 3.1–3.3 | `src/gui.rs` (HTTP client, labels) + the manager window |
| 3.2 | `GET /flip` in `src/main.rs` |
| 4.x | `src/bin/ipp-duplexd-fork-gui.rs` (`--flip` mode), `src/gui.rs` (exit codes) |
| 1.3 | `POST /reload` in `src/main.rs` |

## Deliberate non-goals

- No remote management: the daemon listens on loopback, the manager talks to
  loopback, and CUPS remains the one thing that speaks network IPP.
- No automatic printer discovery (mDNS/Bonjour). The real printer is entered by
  its `ipp://` URI, the same string the daemon would be given on the command
  line; discovery can come later without changing the tree.
- No editing of the manager's own build options (page rendering stays qpdf's
  job), and no per-job settings that IPP already carries.
- The manager does not re-implement the flip dialog's decisions; it only shows
  what the daemon decided.
