# MyKVM

一套键盘、一只鼠标、一份剪贴板，在同一局域网内的 Mac / Windows / Linux 之间共享；**本仓库额外提供无 root 的 Android 被控端**：用电脑的鼠标键盘直接操作手机、平板。

[![下载](https://img.shields.io/github/v/release/PCCCQ/mykvm?label=Download&style=for-the-badge)](https://github.com/PCCCQ/mykvm/releases/latest)
[![平台](https://img.shields.io/badge/平台-macOS%20%7C%20Windows%20%7C%20Linux%20%7C%20Android-2786ff?style=for-the-badge)](https://github.com/PCCCQ/mykvm/releases/latest)
[![许可证: MIT](https://img.shields.io/badge/license-MIT-green?style=for-the-badge)](./LICENSE)

[English README](./README.en.md)

## 与原版的关系

本项目基于开源项目 [XxMinor/mykvm](https://github.com/XxMinor/mykvm)（作者 XxMinor，MIT 许可）二次开发，**协议与用法保持兼容**：

- 一套键鼠 + 剪贴板在 macOS / Windows / Linux 之间双向共享，光标推过屏幕边缘即切换；
- Rust + Tauri 架构，每台机器一个轻量托盘应用；
- 发现走 UDP `47833`（明文），输入与剪贴板走 UDP `47834` 上的加密 QUIC/TLS 连接，并钉扎对端广播的证书；
- 服务端/客户端模式、多显示器布局编辑、文本与图片剪贴板同步、中英双语界面。

## 相比原版的改动

| | 原版 | 本版 |
| --- | --- | --- |
| **Android 端** | 无 | **新增被控端**（手机 / 平板，无 root） |
| **Linux 键鼠** | 仅剪贴板同步 | 完整支持：可作控制端（X11 抓取、贴边吸附、快捷键切屏）与被控端（XTEST 注入）；需 X11 会话 |
| **应用内更新** | 检查并自动安装更新 | 已移除，改为从 Releases 手动下载安装包 |
| **发布流程** | 草稿预创建 + 更新器清单 + 签名校验 | GitHub Actions 直接构建并发布 Release，不再产生草稿 |
| **文档** | 简短，英文为主 | 重写为中文（默认）/ 英文双语 |

## Android 被控端

电脑的鼠标键盘直接操作手机 / 平板，**只接收、不控制其他设备**。

- **无 root**：键鼠注入走 [Shizuku](https://shizuku.rikka.app/)（`shell` 权限调用 `InputManager.injectInputEvent`）。
- **有线 / 无线**：USB 网络共享或同一 Wi-Fi 均可，自动发现 + 验证码配对。
- **两种输入模式**：鼠标模式（原生鼠标事件，支持悬停、真正的右键菜单与滚轮）或触摸模式（合成触摸，兼容所有应用）。
- **键盘**：电脑按键直接输入到平板应用，**且保留平板自己的输入法**，因此中文输入可用。
- **剪贴板**：电脑复制 → 平板粘贴（Android 限制后台读取剪贴板，反向暂不支持）。
- **平板电脑模式**：按整块屏幕分辨率上报，自由窗口下坐标依然准确；光标大小可调。
- **诊断日志**：应用内查看 / 分享 / 清空，并可**一键发送到电脑**，日志落在电脑的日志目录里。

详细安装步骤、权限与排错见 [`android/README.md`](./android/README.md)。

## Android 端优化与修复

- **旋转 / 切换电脑模式不再断连**：平板旋转或进出电脑模式时，屏幕尺寸原地更新，不再重启接收端（重启会掐断 QUIC 连接，电脑端会一直报 "server refused"）。
- **重新配对可用**：已配对的平板仍可接受电脑重新配对；电脑证书轮换或 IP 变化后会自动刷新配对记录，不必两边清空重来。
- **键盘可用**：默认不再关闭平板输入法（旧的"键盘直通"会让平板没有输入法，既不弹软键盘也不能打中文）；该开关仅作为极少数 ROM 的兜底。
- **后台保活**：电池优化白名单、定时唤醒、服务看门狗，锁屏后不易被系统杀掉。
- **一键日志**：安卓端日志含协议层与注入层记录，可一键上传到电脑排查。
- **鼠标**：新增鼠标/触摸模式切换，光标大小可调。
- **剪贴板**：新增电脑 → 平板方向的文本同步。

## 快速开始

1. 从 [Releases](https://github.com/PCCCQ/mykvm/releases/latest) 下载对应平台的安装包并安装。
2. 在要分享键鼠的机器上保持**服务端**（默认），另一台切到**客户端**。
3. 同一局域网内会自动发现；也可以打开**设备**手动输入对方 IP 后点**添加**。
4. 打开**布局**，把显示器拖到与实际桌面一致的贴合位置。
5. 把光标推过共享边缘即可接管另一台机器；键盘跟随，剪切粘贴双向可用。

Android 端：安装 APK → 启动 Shizuku 并授权 → 打开接收服务 → 在电脑端配对（平板显示 6 位验证码）。

## 权限

- **macOS**：系统设置 → 隐私与安全性，授予 **辅助功能** 和 **输入监控**；安装包为自签名未公证，首次需右键 → 打开。
- **Windows**：常规使用无需特殊权限；要控制提权窗口才需要以管理员身份运行。
- **Linux**：需要 X11/Xorg 会话（Wayland 原生会话会给出明确报错）。
- **Android**：需要 Shizuku 与悬浮窗权限；若要"键盘直通"兜底功能，另需 `WRITE_SECURE_SETTINGS`（默认不需要）。

## 构建

```bash
npm install
npm run tauri:bundle   # 桌面安装包
```

Android 安装包（需要 JDK 17 + Android SDK/NDK，Rust 交叉编译由 Gradle 驱动）：

```bash
cd android
./gradlew :app:assembleDebug     # 已签名，可直接安装
./gradlew :app:assembleRelease   # 体积更小，但产物未签名
```

推送 `main` 或手动触发 Release 时，GitHub Actions 会自动构建 APK 并上传到 Release。
配置了签名密钥（`ANDROID_KEYSTORE_BASE64` 等 secrets）时发布签名后的 release 版，
否则发布可直接安装的 debug 版；Pull Request 上则两个变体都会构建并作为构建产物上传。

## 已知限制

- **仅限可信局域网**：发现是明文且未认证，请勿把端口暴露到公网。
- 剪贴板只同步**文本和图片**，不同步文件。
- Android 端剪贴板仅支持电脑 → 平板方向。
- macOS 安装包为自签名、未公证。

## 许可证

MIT，见 [LICENSE](./LICENSE)。原版版权归 [XxMinor/mykvm](https://github.com/XxMinor/mykvm) 所有。
