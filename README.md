# MyKVM

**一套键盘、一只鼠标、一份剪贴板 —— 在同一局域网内的 Mac、Windows、Linux 之间共享。**

光标推到屏幕边缘，就会落到下一台机器上；键盘跟随光标走，剪贴板（文本和图片）自动同步。无需 KVM 硬件和线缆 —— 一个基于 Rust 和 Tauri 的软件 KVM。

[![下载](https://img.shields.io/github/v/release/PCCCQ/mykvm?label=Download&style=for-the-badge)](https://github.com/PCCCQ/mykvm/releases/latest)
[![平台](https://img.shields.io/badge/平台-macOS%20%7C%20Windows%20%7C%20Linux-2786ff?style=for-the-badge)](https://github.com/PCCCQ/mykvm/releases/latest)
[![许可证: MIT](https://img.shields.io/badge/license-MIT-green?style=for-the-badge)](./LICENSE)

[English README](./README.en.md)

## 与原版的区别与联系

**本项目源自开源项目 [XxMinor/mykvm](https://github.com/XxMinor/mykvm)(原版 MyKVM,作者 XxMinor,MIT 许可),在原版基础上进行了重制与扩展。** 承袭了原版的核心设计,大部分行为与协议保持兼容:

- 一套键鼠、一份剪贴板在 macOS / Windows / Linux 之间局域网内共用;
- Rust + Tauri 架构,每台机器一个轻量托盘应用;
- 输入与剪贴板走加密 QUIC/TLS 传输,钉扎对端广播的证书;
- 双通道协议:发现 UDP `47833`,输入/剪贴板 UDP `47834`;
- 服务端/客户端模式、多显示器布局、剪贴板文本与图片同步、简体中文/英文双语界面。

与[原版](https://github.com/XxMinor/mykvm)相比,本版的主要区别:

| 方面 | 原版 | 本版 |
| --- | --- | --- |
| Linux 输入共享 | 仅剪贴板同步 | 完整支持:**控制端**(X11 指针/键盘抓取,贴边吸附、快捷键切屏、多屏漫游)与**被控端**(XTEST 输入注入);要求 X11/Xorg 会话,Wayland 原生会话给出明确报错 |
| 应用更新 | 应用内检查更新并自动安装(更新面板、标题栏徽标) | 已移除;升级需从 [Releases](https://github.com/PCCCQ/mykvm/releases) 手动下载新安装包 |
| 发布流程 | 草稿预创建、并行构建、更新器清单(`latest.json`)与签名校验 | 主分支推送或手动触发,由 GitHub Actions 构建 macOS/Windows/Linux 安装包并发布 GitHub Release |
| 界面 | 含更新面板与更新徽标 | 移除更新相关界面与逻辑,控制台精简 |
| 文档 | 简短,英文为主 | 重写,中文(默认)/英文双语 |

> 注:Linux 输入共享基于纯 Rust 的 x11rb 客户端实现,构建时无需系统 libX11/libXtst 开发包。

## 为什么选择 MyKVM

- **真正的跨平台控制。** macOS、Windows、Linux 机器之间共享一套键鼠，双向可用。
- **完整的 Linux 支持。** Linux 不再只是"能同步剪贴板"：既能作为控制端操控其他机器（X11 指针/键盘捕获，与 Windows/macOS 同款的贴边吸附、多屏漫游、快捷键切屏），也能作为被控端接收远程键鼠（XTEST 输入注入）。
- **多显示器漫游。** 支持排布布局编辑，光标可以在远端机器多块屏幕之间移动，而不只是主屏。
- **加密传输。** 输入和剪贴板走 TLS 1.3（QUIC）连接并绑定对端广播的证书，流量不以明文出现。
- **剪贴板文本与图片。** 在一台机器复制，另一台直接粘贴。
- **轻量。** Rust 后端 + Tauri 外壳，每台机器只需一个小巧的托盘应用。
- **双语界面。** 简体中文和 English。

## 快速开始

1. **两台机器都安装。** 从 [Releases 页面](https://github.com/PCCCQ/mykvm/releases/latest) 下载对应平台的安装包。
2. **选择角色。** 在希望分享键鼠的那台机器上打开 MyKVM，保持 **服务端**（默认）；另一台机器打开 MyKVM，在设置中切到 **客户端**。
3. **连接。** 同一局域网内两台机器会自动互相发现；也可以打开 **设备**，手动输入对方 IP（可选 `IP:端口`）并点击 **添加**。只有上报了屏幕信息的设备才会进入布局。
4. **排布屏幕。** 打开 **布局**，把显示器拖到与实际桌面一致的贴合位置。
5. **跨界。** 把光标推过共享边缘 —— 它会落到另一台机器上。键盘跟随，剪切粘贴双向可用。

## 权限说明

- **macOS（服务端）。** 在 系统设置 → 隐私与安全性 中给 MyKVM 同时授予 **辅助功能（Accessibility）** 和 **输入监控（Input Monitoring）**，这是捕获和注入键鼠输入所必需的。
- **macOS 首次打开。** 版本是免费自签名（未经过 Apple 公证），首次会被 Gatekeeper 拦。右键点应用 → **打开** → **打开** 放行一次即可。
- **Windows。** 常规使用无需特殊权限。只有需要控制提权/管理员窗口时才以管理员身份运行。
- **Linux。** 输入共享需要 **X11/Xorg 会话**：注入依赖 XTEST 扩展、控制远端时隐藏本地光标依赖 XFIXES 扩展（几乎所有 X 服务器都内置）。Wayland 原生会话没有 X 显示，暂时无法共享键鼠 —— 应用会给出明确报错；在 XWayland 下可以运行但全局抓取可能受限，建议使用真正的 X11 会话。
- **Linux 键盘布局。** 按键通过服务器键位表翻译，字母、数字、标点等产生字符的键在混合布局间能正确输入；AltGr 专属符号、死键、媒体键以及依赖 NumLock 状态的小键盘语义暂不翻译（这些事件会被丢弃并记日志）。

## 已知限制

- **仅限可信局域网。** 暂无用户配对/PIN，且局域网发现是明文、未认证。请勿把端口暴露到公网或不可信网络。
- 输入和剪贴板走 **加密的 QUIC/TLS** 连接，并绑定对端广播的证书；但 MyKVM 仍是原型，未针对恶意网络做加固。
- 剪贴板仅同步**文本和图片**，不同步文件。
- macOS 安装包为**自签名、未公证** —— 首次打开会看到 Gatekeeper 提示。
- 实验性软件：协议和行为可能随版本变化。

---

## 功能特性

- 支持 Server / Client 两种工作模式。
- 支持局域网设备发现。
- 支持通过主机名或 IP 手动连接设备。
- 支持本机显示器检测和多显示器布局编辑。
- **双向**共享键盘和鼠标输入（macOS ↔ Windows ↔ Linux），走加密 QUIC 连接。
- 通过同一条加密连接同步剪贴板的文本和图片。
- 支持浅色、深色和跟随系统主题。
- 支持英文和简体中文界面。
- 支持托盘隐藏和恢复主窗口。

## 当前状态

MyKVM 是实验性早期版本，适合本地测试与迭代，但未针对不可信网络加固。当前版本和安装包见 [Releases 页面](https://github.com/PCCCQ/mykvm/releases)。

- 许可证：MIT
- 默认端口：UDP `47833`（发现）和 UDP `47834`（QUIC 传输）
- 剪贴板负载上限：文本 256 KB，图片 32 MB
- 传输安全：输入和剪贴板运行在 TLS 1.3（QUIC）连接上，绑定对端广播的证书
- 安全模型：可信局域网原型
- 尚未包含：用户配对/PIN、认证发现、生产级传输加固

请勿将传输端口暴露到公网或不可信网络。

## 协议

MyKVM 运行两个通道：局域网发现走明文 UDP 端口；输入和剪贴板走第二个 UDP 端口上的加密 QUIC 连接。

| 通道 | 默认端口 | 传输 | 标记 | 用途 |
| --- | --- | --- | --- | --- |
| 发现 | UDP `47833` | UDP 数据报 | `mykvm.discovery.v1` | 局域网发现、探测/应答、主机信息、显示器元数据 |
| 输入 | UDP `47834` | QUIC 数据报 | `mykvm.input.v1` | 鼠标移动、按键、滚轮和键盘事件（低延迟、容忍丢包） |
| 剪贴板 | UDP `47834` | QUIC 流 | `mykvm.clipboard.v1` | 剪贴板文本和图片同步（可靠、有序） |

发现端口可在设置中配置（默认 UDP `47833`）；QUIC 传输端口默认是发现端口 +1（UDP `47834`）。端口被占用时两者都会在邻近端口自动回退，也可使用系统选定的端口。设备之间广播各自的发现端口、QUIC 端口、传输公钥和协议版本，因此自动发现和手动添加的设备都能连对端口、钉住正确的证书。

QUIC 连接是 TLS 1.3 加密的：每台设备启动时生成自签名证书并在发现阶段广播，连接方钉住该证书，输入和剪贴板流量因此加密并与广播的对端绑定。发现本身仍是明文且未认证，请只在可信局域网内使用 MyKVM。

## 开发要求

- Node.js 22+
- Rust stable
- 平台工具链：
  - Windows：Microsoft C++ Build Tools
  - macOS：Xcode Command Line Tools
  - Linux：WebKitGTK 和 appindicator 开发包（`libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev patchelf`）

## 开发

安装依赖：

```bash
npm install
```

启动 Web 界面：

```bash
npm run dev
```

启动 Tauri 桌面应用：

```bash
npm run tauri:dev
```

构建（不打包安装包）：

```bash
npm run tauri:build
```

构建桌面安装包：

```bash
npm run tauri:bundle
```

## 平台辅助脚本

Windows：

```powershell
powershell -ExecutionPolicy Bypass -File .\scripts\check-dev-env.ps1
powershell -ExecutionPolicy Bypass -File .\scripts\run-tauri-dev.ps1
```

macOS 和 Linux：

```bash
sh scripts/check-dev-env.sh
sh scripts/run-tauri-dev.sh
```

## 验证

提交 pull request 或发布前运行：

```bash
npm run build
npm run lint
cargo check --manifest-path src-tauri/Cargo.toml
cargo test --manifest-path src-tauri/Cargo.toml
```

## 发布

Git 只保存源码历史。实际的打包由 GitHub Actions 在 GitHub 托管的 runner 上完成。

发布工作流监听 `main` 分支的推送：

- `feat:` 发布下一个 minor 版本（例如 `v0.1.0` → `v0.2.0`）。
- `fix:` 发布下一个 patch 版本（例如 `v0.1.0` → `v0.1.1`）。
- 其他前缀只跑常规检查，不发布。
- 如果还没有 release tag，第一次 `feat:` 或 `fix:` 推送会发布 `v0.1.0`。

发布说明取自 [CHANGELOG.md](./CHANGELOG.md) 的 `## [Unreleased]` 部分（用户视角文案），缺少时回退到过滤后的提交标题。提交变更时请保持该部分最新。

示例：

```bash
git commit -m "feat: initial desktop release"
git push origin main
```

工作流会创建 git tag，构建 macOS、Windows 和 Linux 安装包，然后用生成好的安装包发布 GitHub Release。

## 项目结构

| 路径 | 用途 |
| --- | --- |
| `src/App.tsx` | 主 React 桌面控制台 |
| `src/desktopApi.ts` | 前端与 Tauri 命令的桥接 |
| `src/layout.ts` | 显示器布局变换与相邻关系逻辑 |
| `src/runtime.ts` | 运行时状态类型 |
| `src-tauri/src/lib.rs` | Tauri 命令、UDP 发现、剪贴板同步、应用状态和性能采样 |
| `src-tauri/src/input.rs` | 输入捕获、转发和注入运行时 |
| `src-tauri/src/linux_input.rs` | Linux X11 后端：指针/键盘捕获、键位表翻译、XTEST 注入 |
| `src-tauri/src/quic_transport.rs` | 加密 QUIC 传输（输入数据报、剪贴板流）与证书钉扎 |
| `scripts/` | 开发与构建辅助脚本 |

## 贡献

欢迎提交 issue 和 pull request。请保持改动聚焦，记录影响协议的行为；改动共享运行时代码时同时验证 Web 构建和 Tauri 后端。Linux 输入相关改动请运行 X11 实测探针：`cargo test -p mykvm -- --ignored x11_backend_probe`。

提交前缀和版本规则见 [CONTRIBUTING.md](./CONTRIBUTING.md)。

## 许可证

MIT，见 [LICENSE](./LICENSE)。
