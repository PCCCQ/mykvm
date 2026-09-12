# MyKVM

One keyboard, one mouse, one clipboard — shared between Mac, Windows and Linux machines on the same LAN. **This fork additionally ships a root-free Android receiver**: drive a phone or tablet with the desktop's mouse and keyboard.

[![Download](https://img.shields.io/github/v/release/PCCCQ/mykvm?label=Download&style=for-the-badge)](https://github.com/PCCCQ/mykvm/releases/latest)
[![Platforms](https://img.shields.io/badge/platforms-macOS%20%7C%20Windows%20%7C%20Linux%20%7C%20Android-2786ff?style=for-the-badge)](https://github.com/PCCCQ/mykvm/releases/latest)
[![License: MIT](https://img.shields.io/badge/license-MIT-green?style=for-the-badge)](./LICENSE)

[中文说明](./README.md)

## Relationship to the Original Project

This project is a fork of [XxMinor/mykvm](https://github.com/XxMinor/mykvm) (by XxMinor, MIT licensed). **The protocol and day-to-day usage stay compatible:**

- One keyboard and mouse plus a shared clipboard across macOS / Windows / Linux; push the cursor off a screen edge and it lands on the next machine;
- Rust + Tauri: one small tray app per machine;
- Discovery on UDP `47833` (plaintext); input and clipboard over an encrypted QUIC/TLS connection on UDP `47834`, pinned to the peer's advertised certificate;
- Server/client modes, multi-monitor layout editing, text and image clipboard sync, English and Simplified Chinese UI.

## What This Fork Changes

| | Original | This fork |
| --- | --- | --- |
| **Android** | Not supported | **New receiver** for phones and tablets, no root required |
| **Linux input** | Clipboard sync only | Full support: can control (X11 capture, edge snap, hotkey switch) and be controlled (XTEST injection); needs an X11 session |
| **In-app updates** | Checks and installs updates in place | Removed; download a new installer from Releases |
| **Release workflow** | Draft precreation + updater manifest + signing checks | GitHub Actions builds and publishes the release directly; no draft is created |
| **Docs** | Brief, English-first | Rewritten, Simplified Chinese (default) and English |

## Android Receiver

Control a phone or tablet with the desktop's mouse and keyboard. **Receive only** — it never controls other machines.

- **No root**: input injection goes through [Shizuku](https://shizuku.rikka.app/) (the `shell` user's `InputManager.injectInputEvent`).
- **USB or Wi-Fi**: USB tethering and the same Wi-Fi network both work, with LAN discovery plus code-based pairing.
- **Two input modes**: mouse mode (native mouse events — hover, real context menus, wheel) or touch mode (synthetic touches, works in every app).
- **Keyboard**: the desktop's keys type straight into the tablet's apps **while leaving the tablet's own IME in place**, so Chinese input works.
- **Clipboard**: desktop → tablet only (Android blocks background clipboard reads in the other direction).
- **Tablet PC mode**: reports the full display size, so coordinates stay correct inside a freeform window; the pointer size is adjustable.
- **Diagnostics**: view, share or clear the log in-app, or **send it to the desktop in one tap**, where it lands next to the desktop's own log.

See [`android/README.md`](./android/README.md) (Chinese) for setup, permissions and troubleshooting.

## Android Fixes and Improvements

- **Rotation / PC-mode changes no longer drop the connection**: the screen size is updated in place instead of restarting the receiver, which used to tear down the QUIC endpoint and make the desktop spam "server refused".
- **Re-pairing works**: an already-paired tablet still accepts a new pairing, and a rotated certificate or a moved IP refreshes the stored record instead of locking input out.
- **The keyboard works**: the "keyboard passthrough" switch is off by default. It used to leave the tablet with no input method at all — no soft keyboard and no Chinese input.
- **Stays alive**: battery-optimisation exemption, a keep-alive alarm and a service watchdog, so an aggressive ROM is less likely to kill it while the screen is off.
- **One-tap logs**: protocol and injection records in one file, uploadable to the desktop.
- **Mouse**: mouse/touch mode switch and an adjustable pointer size.
- **Clipboard**: desktop → tablet text sync.

## Quick Start

1. Download the installer for your platform from [Releases](https://github.com/PCCCQ/mykvm/releases/latest).
2. Keep **server** (the default) on the machine whose keyboard and mouse you want to share; switch the other to **client**.
3. Machines on the same LAN find each other automatically; you can also open **Devices**, type the peer's IP and press **Add**.
4. Open **Layout** and drag the displays into the position they physically sit in.
5. Push the cursor past the shared edge to take over the other machine. The keyboard follows, and copy/paste works in both directions.

Android: install the APK → start Shizuku and grant permission → enable the receiver → pair from the desktop (the tablet shows a 6-digit code).

## Permissions

- **macOS**: System Settings → Privacy & Security, grant both **Accessibility** and **Input Monitoring**. The build is self-signed and not notarised, so the first launch needs right-click → Open.
- **Windows**: nothing special for normal use; run as administrator only to control elevated windows.
- **Linux**: requires an X11/Xorg session (Wayland-native sessions report a clear error).
- **Android**: Shizuku plus overlay permission. The "keyboard passthrough" fallback additionally needs `WRITE_SECURE_SETTINGS` (not needed by default).

## Building

```bash
npm install
npm run tauri:bundle   # desktop installer
```

Android APK (needs JDK 17 plus the Android SDK/NDK; Gradle drives the Rust cross-build):

```bash
cd android
./gradlew :app:assembleDebug     # signed, installable as-is
./gradlew :app:assembleRelease   # smaller, but the artifact is unsigned
```

Pushing to `main` or dispatching the Release workflow makes GitHub Actions build the
APK and attach it to the release. With signing secrets configured
(`ANDROID_KEYSTORE_BASE64` and friends) it publishes the release-signed APK,
otherwise it publishes the installable debug build; pull requests build and upload
both variants as workflow artifacts.

## Known Limitations

- **Trusted LAN only**: discovery is plaintext and unauthenticated; never expose the ports to the internet.
- The clipboard syncs **text and images**, not files.
- The Android clipboard only flows desktop → tablet.
- macOS builds are self-signed and not notarised.

## License

MIT, see [LICENSE](./LICENSE). The original copyright belongs to [XxMinor/mykvm](https://github.com/XxMinor/mykvm).
