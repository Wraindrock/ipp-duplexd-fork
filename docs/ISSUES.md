# Issue 整理（Wraindrock/ipp-duplexd-fork）

抓取时间 2026-10-28 20:5x，`gh issue list --state all` 共 3 条，全部 OPEN，全部由 Wraindrock 提出，无标签、无里程碑。
下面的“核查”是对照仓库代码的静态阅读结果（当天本机没有 cargo，未编译、未跑测试、未动真机）。

**更新（2026-10-28 晚）**：`#1` 评论里那条“GUI 首次运行无法启动服务”已经做完——`src/service.rs` 现在能写 unit、补 `.conf`、`enable --now`，窗口里有了首次运行入口，`cargo test` / `clippy --features gui` 全绿，并在真机上以 `IPP_DUPLEXD_FORK_GUI_TEST=setup` 实跑验证过。详见文末“已做”。

## 一览

| # | 标题 | 实际类型 | 一句话结论 | 建议 |
| --- | --- | --- | --- | --- |
| 1 | 需要 ipp-usb，否则无法打印 | 未文档化的运行时依赖（+ body 已过期） | 代码没问题，缺的是“依赖 ipp-usb”的文档/打包声明；评论里另有“GUI 首次运行无法启服务”这一独立缺口 | 先改文档与包依赖，再单开 GUI 首启 issue |
| 2 | 错误缩放问题 | 真 bug（在 CUPS 队列层，不在守护进程） | `lpadmin -m everywhere` 建队列时若真机不可达，守护进程只上报 A4/Letter，PPD 没 B5，pdftopdf 就把 B5 归一化成 A4 | 改 fallback 媒体列表 + 建队列时机，加一条 B5 端到端用例 |
| 3 | GUI 设计思路 | 设计任务（能力清单 → 界面对齐） | 窗口只覆盖了后端能力的一部分：6 个 per-printer 旋钮、服务安装、任务列表都缺 | 按本文的能力对照表逐项补 |

---

## #1 需要 ipp-usb，否则无法打印

**原文**：正文只有两行命令 `systemctl stop/disable ipp-usb`，与标题相反。
**评论（2026-10-28 06:08）才是真结论**：

> 原因是 ipp-usb 与 hplip 冲突，争夺 usb 占用。决策：放弃 hplip，转而使用 ipp-usb。
> 另外：当前纯 GUI 首次运行无法启动服务，必须使用 TUI，目标是只用 GUI 就足够，后续逐步移除 TUI。

**核查**

- 守护进程本身不碰 USB：它监听 TCP 端口讲 IPP，把活转给 `printer.real` 指向的 IPP 端点（`src/main.rs:1730` `forward`）。USB 打印机的 `ipp://` 端点正是 ipp-usb 提供的，所以“需要 ipp-usb”成立，与代码无冲突。
- 全仓库搜不到 `ipp-usb` / `hplip` 字样（README、docs、packaging 都没有），包依赖里也没有：
  - `packaging/rpm/ipp-duplexd-fork.spec:48-51`：`Requires: qpdf gtk4 libadwaita`，只 `Recommends: cups-client`
  - `packaging/build-deb.sh:54-55`：`Depends: qpdf libc6 libgtk-4-1 libadwaita-1-0`，`Suggests: cups-client`
  - `packaging/arch/PKGBUILD:16`：`depends=('qpdf' 'gcc-libs' 'gtk4' 'libadwaita')`，无 `optdepends`

**判断**：不是代码 bug，是两件事被混在一条 issue 里——

1. 依赖没写清楚，用户装完（尤其是卸了 hplip 之后）会以为坏掉；
2. body 是过期/反向的（写的是“停掉 ipp-usb”），后来的读者会被误导。

**待办**

- 重写 body：讲清“本程序只讲 IPP，USB 打印机要靠 ipp-usb 暴露成 `ipp://localhost:60000/ipp/print`；hplip 会抢 USB，两者二选一”。
- 打包补依赖：rpm `Recommends: ipp-usb`、deb `Recommends: ipp-usb`、PKGBUILD `optdepends=('ipp-usb: ...')`；README 的 Requirements 加一节。
- 可选的运行时提示：`printer.real` 指向 `localhost` 但 `ipp-usb` 不在跑时，状态页/窗口给一条“没检测到 ipp-usb”的提示，而不是干等超时。
- 评论里那条“GUI 首次运行起不了服务”不属于本 issue，已单独做完（见文末“已做”）。
- 2026-10-28：issue #1 的正文已重写成可独立跟进的任务单（背景、证据、任务、验收、验证命令、不在范围）；后续以 GitHub 上的 #1 为准，本文只保留结论与批评。

---

## #2 错误缩放问题

**原文**：虚拟打印机在打印时选的纸张（A4/B5…）在 CUPS 递给真机时被改掉；期望本程序只做“拆正反面 + 用户确认后打背面”，纸张原样透传。

**核查：守护进程不是改尺寸的那一层**

- `forward()`（`src/main.rs:1730`）转发客户端 job 属性时只剔除 `NO_FORWARD` 的 8 项——`sides`、`copies`、`page-ranges`、`number-up`、`number-up-layout`、`page-border`、`multiple-document-handling`、`job-hold-until`（`src/main.rs:1719`）。`media` / `media-col` 不在其中，会原样转给真机。
- qpdf 只做分页与旋转：`--pages … 1-z:odd`、`--pages … even`、`--rotate=+180:1-z`（`src/main.rs:1640`），没有 `--page-size` / `--scale`。
- 所以“纸张被改”发生在更上游：**CUPS 虚拟队列那一层**。

**关键嫌疑（可复现的链条）**

1. 队列用 `lpadmin -p NAME -E -v ipp://ADDR/ipp/print -m everywhere` 建立（`src/queues.rs:38-43`）。`-m everywhere` 的 PPD 是**建立队列那一刻**向该 URI 查询 `Get-Printer-Attributes` 生成的。
2. 这个 URI 指向守护进程，而守护进程的属性是“代理真机 + 覆盖少数项”得到的：`proxied_printer_attrs()`（`src/main.rs:996`）转发真机的 `media-supported`/`media-default`（`apply_overrides` 在 `src/main.rs:1069` 的覆盖清单里**不含** media——这是对的）。
3. 但真机不可达时，守护进程退到 `fallback_printer_attrs()`（`src/main.rs:1038`），其中 `media-supported` 硬编码为 `iso_a4_210x297mm` + `na_letter_8.5x11in`，`media-default` 为 A4（`src/main.rs:1057-1060`）。
4. 于是 PPD 里只有 A4/Letter，没有 B5；CUPS 的 `pdftopdf` 按“页面尺寸不在 PPD 就归一化/缩放”的规则把 B5 变成 A4 —— 正好是“改动的本意是好的”（保证能打），但与“原样透传”冲突。
5. 更糟的是刷新时机：窗口每次启动都调 `register_queues()`（`src/bin/ipp-duplexd-fork-gui.rs:520`），它对每个打印机重跑 `queues::ensure()`（`src/bin/ipp-duplexd-fork-gui.rs:527`），即**重新生成 PPD**；而它的门槛只检查“守护进程答不答”（`http_get(addr, "/")`），**不检查真机在不在线**。所以“真机电源关着时打开一次窗口”就足以把已有 B5 的队列降级成 A4/Letter。

**验证手段**（在有真机的机器上，一分钟内能判定）

```console
lpoptions -p <queue> -l | grep -i -E 'b5|a4'      # PPD 里有没有 B5
lpstat -l -o <queue>                              # 实际 job 的 media
# 直接问守护进程的真机属性
ipptool -tv ipp://127.0.0.1:<port>/ipp/print get-printer-attributes.test | grep media
```

**修法方向**（按性价比排序）

1. **不要让 fallback 决定队列能力**：真机不可达时 (`fallback_printer_attrs`) 不要把 `media-supported` 缩成两种纸；更稳的做法是不上报或上报“已知全集”（例如上次成功时的缓存，落盘到配置目录），至少不要硬编码 A4/Letter。
2. **建队列前先确认真机可达**：`register_queues` 的门槛从“守护进程答不答”改成“真机可达”（守护进程本来就代理真机状态，可以扩展 `/` 或加 `/health` 返回 online 字段），不可达就跳过 `lpadmin`，保留旧 PPD。
3. **纸型变化后刷新 PPD**：真机换纸/换了型号时，需要重新 `lpadmin -m everywhere`；现在只有地址变化才重跑（`queues::ensure` 是幂等的，天然支持，但要由触发条件调用）。
4. **回归用例**：`tests/` 里加一条端到端——提交 `media=iso_b5_176x250mm` 的作业，断言转发给真机的 IPP 请求里 `media` 仍是 B5；再补一条“真机离线时 `Get-Printer-Attributes` 不应把 B5 从 `media-supported` 里抹掉”。

**验收标准**：GTK 打印对话框选 B5 → 真机面板/`lpstat -l -o` 显示 B5，且 PDF 页面框尺寸未变。

---

## #3 GUI 设计思路

**原文**：要求“前后端完全对应”——先把后端真实能力列成清单，每项对应界面上一个具体按钮/字段，再按清单逐项补齐对齐。

**核查：界面上现在有什么**（`src/bin/ipp-duplexd-fork-gui.rs`）

| 区域 | 现有内容 |
| --- | --- |
| 标题栏 | 标题 + 副标题；`+` 按钮（`list-add-symbolic`）打开新建对话框 |
| Virtual printers 组 | 每行：名称、`listen → real` 副标题；4 个图标按钮 = 打印背面（`media-playback-start-symbolic`）、查看状态（`zoom-in-symbolic`，弹纯文本状态）、编辑、删除（带确认对话框） |
| Service 组 | unit 名 + `systemctl --user is-active` 文本 + `Restart` 按钮 |
| Settings 组 | 语言下拉（`l10n::codes()`）、配置文件路径（只读） |
| About 组 | 版本、项目链接 |
| 编辑对话框 | “检测到的打印机”列表（`discover`，点一下自动填）+ 可折叠 `Advanced`（name / listen / real 三个 `EntryRow`） |

**能力 vs 界面的缺口**

| 后端能力（代码位置） | 界面现状 | 差距 |
| --- | --- | --- |
| `Printer.blank`（leading/trailing/none，`src/config.rs:42`） | 无 | 需 ComboRow |
| `Printer.settle`（秒，`src/config.rs:45`） | 无 | 需 SpinRow |
| `Printer.poll`（秒，`src/config.rs:48`） | 无 | 需 SpinRow |
| `Printer.rotate_even`（0/180，`src/config.rs:51`） | 无 | 需 SwitchRow |
| `Printer.auto_continue`（秒，`src/config.rs:54`） | 无 | 需 SpinRow（0 = 一直等） |
| `Printer.gui`（是否弹确认框，`src/config.rs:57`） | 无 | 需 SwitchRow |
| 服务安装/启用（`setup_service()`，`src/bin/ipp-duplexd-fork-setup.rs`） | **已补**：`service::install` + 首次运行行（`src/service.rs`） | 已能纯 GUI 首启；细节见“已做” |
| 队列注册（`queues::ensure`，`src/queues.rs`） | 启动时自动跑一次 | 不能手动重试、失败只有一条 toast |
| 任务列表/进度（IPP `Get-Jobs`、`/` 状态页） | 单次弹窗看文本 | 没有持续列表、没有取消 |
| 真机在线状态（`printer_state`，`src/main.rs:1787`） | 状态弹窗里间接可见 | 没有行级徽标 |
| `qpdf` / `ipp-usb` 依赖检查（`run()` 只查 qpdf，`src/main.rs:157`） | 无 | 首启应给出可执行的修复提示 |
| 日志 | 无 | 可选 |

**建议的落地顺序**

1. 编辑对话框补齐 6 个旋钮（`adw::ComboRow` / `SpinRow` / `SwitchRow`，放进现有 `Advanced` 展开区之上，按“常用/高级”分组）——这是“前后端一一对应”里成本最低、收益最直接的一段。
2. 任务视图：`AdwViewSwitcher` + 两个页（Printers / Jobs），Jobs 页读 IPP `Get-Jobs`，每行可取消；这会让“查看状态”弹窗退化成辅助手段。
3. 首启引导：**第一步已做**（装 unit → 启动 → 等应答 → 建队列）；剩下的“空配置时进一个引导流程”仍可做（现在是“添加打印机”顺带完成安装）。
4. 之后按上表逐行补，每补一行就把“按钮 → 后端调用 → 测试”绑定在一起，避免界面再次跑在能力前面。

---

## 三条之间的关系

- **#1 与 #2 耦合**：ipp-usb 没起来 → 真机（`localhost:60000` 之类）不可达 → 队列拿到 `fallback_printer_attrs()` 的 A4/Letter PPD → 用户选 B5 被归一化。修好 #1 的依赖声明与探测，#2 的触发概率同步下降；但 #2 仍需按上面独立修（真机断电、换纸都会单独触发）。
- **#1 的评论与 #3 同源**：都是“GUI 还不足以独立完成一次完整安装”，#3 的清单里应把“服务安装 + 依赖健康检查”列为第一梯队。
- 三条都不涉及 `ipp-duplexd` 上游（`SamiKalliomaki/ipp-duplexd`）——它们是这个 fork 自己的产品化问题，提交信息可以都以 `fork:` 前缀区分。

## 已做（2026-10-28 晚）：GUI 首次运行能装服务

目标：一台什么都没配的机器，只用窗口就能装好并能打印，不再需要终端向导。

| 位置 | 变化 |
| --- | --- |
| `src/service.rs` | 新增 `installed()` / `packaged_unit()` / `unit_text()` / `daemon_binary()` / `install()`：写 `~/.config/systemd/user/ipp-duplexd-fork.service`（包装的 unit 存在就不写，免得遮蔽包）、缺 `.conf` 时补一个只有注释的文件、`systemctl --user daemon-reload` + `enable --now` + `restart`；`systemctl` 现在统一走一个 helper，并可用 `IPP_DUPLEXD_FORK_SYSTEMCTL` 打桩 |
| `src/bin/ipp-duplexd-fork-gui.rs` | 顶部新增“这台电脑”组（仅当 unit 不存在或没跑时出现），按钮一次性完成安装→等守护进程应答→注册队列；保存第一台打印机时若没有 unit 也自动走这条路；Service 行在未安装时显示“还没安装” |
| `src/bin/ipp-duplexd-fork-setup.rs` | 不再自己拼 unit 文本，改用 `service::unit_text` / `service::daemon_binary`，两个前端不会写出两种 unit |
| `packaging/ipp-duplexd-fork.service` | `EnvironmentFile=-%h/...`：文件不存在不再导致 unit 加载失败（之前只有 README 让用户手动 `cp` 示例才能启动） |
| `locales/{en,zh-CN}.yml` | 6 条新文案 + `service_not_installed` |
| `tests/service.rs`（新） | 4 条：首次安装写的文件与调用、包装 unit 不被覆盖、装两次无副作用、找不到守护进程时报错 |
| `.github/workflows/ci.yml` | 新增 Xvfb 步骤 `IPP_DUPLEXD_FORK_GUI_TEST=setup`：临时 HOME + systemctl 打桩，断言 unit、`.conf` 和 `enable --now` 都发生 |
| `README.md` | 首启说明 + 窗口功能树里的“First run”一条 |

**卸载（同一批，第二轮）**：`service::uninstall()` 停用并删除窗口自己写的 unit（软件包的 unit 只停不删），`~/.config/ipp-duplexd-fork.conf` 只在内容仍是那段注释时才删（用户改过就留下）；打印机、CUPS 队列、`config.toml` 全部保留。窗口的 Service 行多一个“卸载”按钮，确认后执行。这样“卸载 → 安装服务”可以反复跑，用来在一台已经装过的机器上测首启。CI 的 Xvfb 步骤现在连跑两遍（setup 再 uninstall），断言 unit 出现、再消失。

验证：`cargo fmt --check`、`cargo clippy --all-targets -- -D warnings`、`cargo clippy --all-targets --features gui -- -D warnings` 全绿；`cargo test` 与 `cargo test --features gui` 全绿（lib 45、e2e 11、i18n 14、`tests/service.rs` 7、`tests/gui.rs` 3）。本机实跑过完整循环：`setup` → unit 与 `.conf` 出现；`uninstall` → 两者消失且 `disable --now` 被调用；再 `setup` → 两者回来。

两个与环境有关的注意点：

- 在有桌面会话的机器上跑测试要 `env -u DISPLAY -u WAYLAND_DISPLAY XDG_RUNTIME_DIR=/tmp/empty`，否则 `gui_dialog_continues_job` 会挂 25 秒：GUI helper 在这种环境里返回 1 而不是“打不开显示”的 2，守护进程因此不会回退到 zenity。这个问题在未改动的 HEAD 上同样存在（worktree 复现过），与本次改动无关，但值得单开一条 issue。
- 未验证的部分：真实 `systemctl --user enable --now` 的启动（本机没有可用的用户 systemd 会话）与窗口的实际观感（只在 X11/Wayland 上闪过一次，未截图）。

## 建议的优先级

1. **#1**（文档 + 打包依赖）：半天，零风险，直接消除“装了不能用”的支持成本。
2. **#2**（fallback 媒体列表 + 建队列门槛 + B5 回归用例）：需要真机验证，但根因已定位。
3. **#3**（旋钮 → 任务页 → 首启引导）：首启那一步已完成，剩下旋钮与任务视图。

## 实机反馈（2026-10-28 22:00，bluefin VM）

三个症状，一起修了：

1. **服务显示 active，但端口没应答**。守护进程没有 qpdf 时会立刻退出，systemd 每 5 秒重启一次，`is-active` 在两次重启之间仍然报 active。窗口现在先检查 qpdf，缺了就不装，直接给安装命令。
2. **AppImage 装出来的 unit 指向 `/tmp/.mount_*`**。窗口退出后挂载消失，服务随即失效，也是“反复安装”失败的一种来源。现在 AppImage 会先把自己复制到 `~/.local/share/ipp-duplexd-fork/`，unit 指向那份稳定副本（执行 `…AppImage daemon`），卸载时一并删除。
3. **提示被截断、无法复制、没有日志入口**。新增“诊断”组与对话框：unit 的 `ExecStart=`、`systemctl --user status`、最近 80 行 journal、每个端口的探测结果，整块可选中并有“复制”按钮；安装后端口不应答时自动弹出。toast 只留成功提示。
4. 顺带：上次安装没应答时，`probe_failed` 会让“这台电脑 / 安装服务”那行继续显示，可以再试一次，而不是看起来装好了。

仍待确认：VM 上是否真的缺 qpdf（诊断对话框里的 journal 会直接写出来）。

## GitHub 上的任务单（2026-10-28 晚）

三条原 issue 的正文都已重写成可独立接手的形式（背景、证据与行号、任务清单、验收标准、验证命令、不在范围），后面新加的两条同规格。**后续以 issue 为准**，本文只保留结论与批评。

| # | 标题 | 需要什么 |
| --- | --- | --- |
| [#1](https://github.com/Wraindrock/ipp-duplexd-fork/issues/1) | ipp-usb 是未声明的依赖：打包声明、README、运行时提示 | 无 |
| [#2](https://github.com/Wraindrock/ipp-duplexd-fork/issues/2) | 纸张尺寸被改动：真机离线时队列 PPD 丢掉 B5 | 真机确认 |
| [#3](https://github.com/Wraindrock/ipp-duplexd-fork/issues/3) | GUI 与后端能力对齐：六个旋钮、任务列表、队列重试 | 无 |
| [#4](https://github.com/Wraindrock/ipp-duplexd-fork/issues/4) | GUI 助手在无法显示时返回 1 而不是 2，守护进程因此不回退 zenity | 无（有桌面会话的机器上可直接复现） |
| [#5](https://github.com/Wraindrock/ipp-duplexd-fork/issues/5) | 把 qpdf 静态编进 AppImage（提案） | 无，需要决定是否做 |

## 已做（2026-10-28 深夜）：#1 的 ipp-usb 依赖声明与提示

| 位置 | 变化 |
| --- | --- |
| `packaging/rpm/ipp-duplexd-fork.spec` | `Recommends: ipp-usb`，与 `cups-client` 同组 |
| `packaging/build-deb.sh` | control 里 `Recommends: ipp-usb` |
| `packaging/arch/PKGBUILD` | `optdepends=('ipp-usb: expose USB printers as IPP printers …')` |
| `packaging/build-rpm.sh` | 产物检查多一条 `rpm -qp --recommends` 断言——弱依赖不在 `-qpR` 里，只能这样查 |
| `README.md` | 新小节「USB printers」：本程序只讲 IPP、`ipp://localhost:60000/ipp/print` 是 ipp-usb 给的、hplip 为什么不能同时跑（抢同一个 USB 设备）；Packages 一节点一句 |
| `src/usb.rs`（新） | 纯函数：`is_endpoint`（loopback 且 60000 才算）、`installed_in`（查 `PATH`，不启动进程）、`missing`（第一条需要 ipp-usb 的 `real`）；单测覆盖 631 队列、远端 `:60000`、装了 ipp-usb 三种“不该报警”的情况 |
| `src/main.rs` | 状态页 `GET /` 满足条件时多一行，给出三种发行版的安装命令 |
| `src/bin/ipp-duplexd-fork-gui.rs` | 诊断对话框在对应打印机下面加同一行 |
| `locales/{en,zh-CN}.yml` | `common.ipp_usb_missing`（中英各一条） |

验证：`cargo fmt --check`、`cargo clippy --all-targets`、`cargo test`（lib 51、e2e 12、i18n 14、`tests/service.rs` 9）全绿；`--features gui` 的编译、测试和打包先走 CI（本机当时没有 gtk4-devel，只有运行时库；后面 #4 那节补了本地编法）。状态页手动跑了五种组合：本机 `60000` + 无 ipp-usb 出提示（中英各一次），`127.0.0.1:631` 队列、远端 `:60000`、装了 ipp-usb 本机 `:60000` 都不出。

## 已做（2026-10-28 深夜）：#4 对话框没显示出来时返回 2

症状：会话看起来是连通的（`XDG_RUNTIME_DIR` 里有 socket，`adw::init()` 成功），但窗口从来没画出来时，助手退出码是 1。守护进程把 1 当成“用户说还没翻”，于是不再尝试 zenity/kdialog，作业一直等着，日志里什么都没有。

根因（本机实测那条路径）：`run_flip_dialog` 的返回值初值就是 `EXIT_DECLINE`(1)，`app.run_with_args` 在没收到任何 response 时也照样返回。这条路径上 `activate` 跑了、`map` 信号发了、只 tick 了一帧，约 300 ms 后主循环自己结束，`response` 和 `closed` 一个都没发。

修法：只有收到对话框的 response 才算“有人看到过它”，否则返回 2（`EXIT_NO_DISPLAY`，和“打不开显示”同一个码，守护进程因此继续试下一个候选）。测试钩子（`IPP_DUPLEXD_FORK_GUI_TEST`）是直接调回调、不发 response，所以保留它自己的答案。

验证：本机（无合成器、`XDG_RUNTIME_DIR` 活着）`--flip` 从 1 变 2；`XDG_RUNTIME_DIR=/tmp/empty` 与 `GDK_BACKEND=x11 DISPLAY=:44` 仍是 2；`cargo test --features gui` 本机全绿（51+12+3+14+9），`gui_dialog_continues_job` 的 zenity 回退照常；CI 的 GTK4 dialog job 里 xvfb 下 accept/decline 仍是 0/1。

顺带：为了在这台没有 gtk4-devel 的机器上本地跑 `--features gui`，把 Fedora 的 `gtk4-devel`/`libadwaita-devel` 依赖链下下来解到 `/tmp/sysroot`，用 `PKG_CONFIG_SYSROOT_DIR` + 补 `lib*.so` 软链来编译链接；这只是临时环境，仓库里没有因此改动构建方式。


