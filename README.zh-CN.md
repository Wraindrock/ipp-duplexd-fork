# ipp-duplexd-fork —— 用手动双面打印把任何单面打印机变成双面机

[English](README.md) · **简体中文**

> 这是一个 [ipp-duplexd][upstream] 的分支，增加了多语言支持和图形界面管理器。
> Fedora 上可以直接从 [GitHub 托管的软件源][repo] `dnf` 安装；其他发行版用
> [打包脚本](#打包与安装)自己构建。

一个单文件的小型 Rust 守护进程（依赖只有 [rust-i18n][i18n]，外加可选的
GTK4 + [libadwaita][adw] 前端），把任何单面打印机变成手动双面打印机。它自己
就是一个 IPP 打印机，只监听 `127.0.0.1`：把文档发给它，它会先用真机打**奇数页**，
停下来等你翻纸，再把**偶数页倒序**打在背面。

不用 PPD，也不用 CUPS 后端——它以现代 driverless（IPP Everywhere）队列的形式注册，
也就是 CUPS 3.x 要求的方向。

```
应用 --> CUPS 队列（driverless）--> ipp-duplexd-fork (127.0.0.1:6632)
                                       |  qpdf 拆页
                                       |--> 真机：奇数页 1,3,5,…
                                       |    （轮询到打完，进入 media-needed）
                                       |<-- 你翻纸，GET /flip
                                       '--> 真机：偶数页倒序 …,4,2
```

完整英文文档见 [README.md](README.md)；维护者文档在 [docs/](docs/README.md)。
下面是一份中文快速上手。

## 安装

```sh
# Fedora：加一次软件源，之后正常升级
sudo dnf config-manager addrepo --from-repofile=https://wraindrock.github.io/ipp-duplexd-fork/ipp-duplexd-fork.repo
sudo dnf install ipp-duplexd-fork
```

其他发行版：

- **Arch**：`packaging/PKGBUILD`
- **AppImage**：`./packaging/build-appimage.sh`（不安装、不注册，适合试一下）
- **deb / rpm**：见 [Packages](README.md#packages)

## 首次设置

装好后两种前端随便挑一个，**它们写同一个配置文件**、操作完全对应：

- **窗口**：应用菜单里打开 `ipp-duplexd-fork`
- **终端菜单**：`ipp-duplexd-fork-setup`（适合没有桌面的机器、SSH、脚本）

添加第一台虚拟打印机时，程序会顺手装好 systemd 用户服务、注册 CUPS 队列。
之后在打印对话框里就会多出一台打印机（默认名字是 `<真机队列>-duplex`）。

## 打印

普通打印即可，勾选手动双面。打完奇数页会弹出翻纸对话框：

- **确定，打印背面** —— 你已经把纸翻好、放回纸盒
- **还没翻** —— 对话框关掉，去准备好；之后可以在窗口的"作业"页点 ▶ 继续

桌面对话框打不开时，守护进程会依次退回 `zenity`、`kdialog`、`xmessage`，
再不行就一直等你在窗口里点继续。

每台虚拟打印机可以写一条**备注**，只在翻纸对话框里显示，用来提醒自己这台机器
该怎么翻纸。

## 打印机驱动：driverless 优先，hplip 兜底

窗口里每行会标出当前驱动：

- `· IPP` —— 走 ipp-usb / driverless，**这是推荐路径**
- `· hplip` —— 走 hplip 的 `hp:/usb/…`，能用但属于兜底

行尾那个"同步"图标是**一个切换按钮**：按当前驱动把队列切到另一种，旧设备地址和
PPD 会存下来，随时能切回去。

USB 打印机接在不同机器之间切换（比如宿主和虚拟机通过 USB 直通共用一台）时，
窗口会分辨三种情况并分别提示：

- 设备不在这台机器上 → 提示"这台机器上没有接这台打印机"
- ipp-usb 在跑但端口没应答 → 提示并给出修复入口
- ipp-usb 正常但队列还是 hplip → 提示冲突，点一下即可切到 IPP

## 常见问题

**打印没反应**：先在窗口"日志与状态"里看那份报告——它包含 unit 内容、
`systemctl --user status`、最近的 journal，以及每个端口是否应答。

**窗口更新了、行为没变**：服务启动的是
`~/.local/share/ipp-duplexd-fork/ipp-duplexd-fork.AppImage`，和桌面快捷方式更新的
不是同一个文件。窗口会在"服务"分组里自己提示"服务还在跑旧版本的副本"，点
**更新并重启**即可。

**CUPS 把队列禁用了**：连续失败后 CUPS 会自动 `disable` 队列，用
`cupsenable <队列名>` 恢复；如果 hplip 后端卡死，`systemctl restart cups`。

**ipp-usb 端口不是 60000**：它会从 60000 往上找空闲端口，程序会自动探测
60000–61000，并在启动时把队列指到真正应答的端口。

## 语言

界面、翻纸对话框、终端菜单、状态页都有英文和简体中文。用 `--lang CODE`
指定，或交给环境变量（`IPP_DUPLEXD_FORK_LANG`、`LANGUAGE`、`LC_ALL`、
`LANG`，先匹配到的生效）。细节和"如何新增一门语言"见
[README.md 的 Languages 一节](README.md#languages)。

## 打包与安装

```sh
./packaging/build-appimage.sh     # 单文件 AppImage（试一下用）
./packaging/release.sh            # 完整发布：检查、构建、提交、打标签
```

发版规则：版本号是 `YYYY.MM.DD-N`，由 `packaging/version.sh` 生成，不要手改标签；
打标签后由 GitHub Actions 构建 AppImage / deb / rpm / Arch 包并更新 dnf 源。

[upstream]: https://github.com/OpenPrinting/ipp-duplexd
[repo]: https://wraindrock.github.io/ipp-duplexd-fork/
[i18n]: https://github.com/longbridgeapp/rust-i18n
[adw]: https://gnome.pages.gitlab.gnome.org/libadwaita/
