# Documentation

The user guide is the [README](../README.md); everything else is for whoever
builds, packages or fixes the program. Both front ends (the GTK4 window and the
terminal menu) are described there from the user's side; the design behind them
is here.

| Document | Who it is for | What it answers |
| --- | --- | --- |
| [README.md](../README.md) | people printing with it | install, setup, print, flip, options, languages |
| [README.zh-CN.md](../README.zh-CN.md) | the same, in Chinese | 快速上手：安装、设置、打印、翻纸、排错 |
| [OPERATIONS.md](OPERATIONS.md) | both front ends, by their makers | the operation tree: every action, in the order the window shows it |
| [INTERACTION.md](INTERACTION.md) | whoever changes a front end | why there is one core and three front ends, and what each owns |
| [I18N.md](I18N.md) | whoever writes a message | the catalogs, the key namespaces, the tests, adding a language |
| [CHANGE-MAP.md](CHANGE-MAP.md) | whoever changes the code | what else has to change with it, and how to check |
| [DEVELOPMENT.md](DEVELOPMENT.md) | contributors | the toolchain recipe, the checks, the release procedure |
| [UI-FRAMEWORK.md](UI-FRAMEWORK.md) | the curious | why GTK4 + libadwaita |
| [ISSUES.md](ISSUES.md) | maintainers | the running log of what was reported, decided and shipped (Chinese) |

## How these documents are kept

- **One fact, one place.** The options table lives in the README because users
  read it; a change to an option starts there and is then traced through
  [CHANGE-MAP.md](CHANGE-MAP.md).
- **The user guide is bilingual, the rest is not.** The program speaks English
  and Simplified Chinese, so the README has a Chinese companion. Everything
  under `docs/` is written for maintainers in English; `ISSUES.md` is the
  exception and has always been kept in Chinese.
- **Dates are facts.** Where a behaviour changed because of a bug, the entry in
  [ISSUES.md](ISSUES.md) carries the date, the version and the commit range; the
  other documents describe the present.
- **Links are relative.** These files are read on GitHub, in an editor and
  from a checkout, so no absolute paths and no anchors that only work in one
  renderer.
