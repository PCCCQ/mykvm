# Changelog

This file feeds the GitHub Release notes. Keep entries user-facing: describe what
changed for someone *using* MyKVM, not the internal/CI plumbing. The release
workflow publishes whatever is under `## [Unreleased]`, so move those entries
under a version heading when you cut a release (or just leave them — the next
release will reuse them).

## [Unreleased]

### Added

- **Android receiver.** A phone or tablet can now be *controlled* by the desktop: mouse and keyboard drive it over USB tethering or Wi-Fi, with no root needed (input goes through Shizuku). Pick a mouse mode (native hover, context menus, wheel) or a touch mode (works in every app); pair with the 6-digit code shown on the device, and copy text from the desktop straight into the tablet.

- **Linux input sharing (X11).** Linux machines can now both control other
  machines (edge-crossing capture via pointer/keyboard grabs, with the same
  edge-snap, hotkey switch and clipboard follow as Windows/macOS) and be
  controlled (XTEST injection of mouse, keys, wheel and side buttons, plus
  ReleaseAll so a session teardown never leaves stuck keys). Requires an
  X11/Xorg session; Wayland-native sessions show a clear error instead of
  silently doing nothing.

### Fixed

- **Android: the tablet's keyboard no longer disappears.** The "keyboard passthrough" toggle used to leave the tablet with no input method at all — no soft keyboard, no way to switch IMEs, no Chinese input. It is off by default now, and no longer needed: with the tablet's own IME active the desktop's keys type normally, Chinese candidates included.

- **Android: rotation and PC-mode changes no longer drop the connection.** The screen size is updated in place instead of restarting the receiver, which used to tear down the encrypted endpoint and make the desktop report "the server refused to accept a new connection".

- **Android: re-pairing works again.** An already-paired tablet accepts a fresh pairing, and a rotated certificate or a moved LAN address refreshes the stored pairing record instead of silently rejecting every input packet.

- **Android: the receiver survives aggressive power management.** A battery-optimisation exemption, a keep-alive alarm and a service watchdog keep it running with the screen off.

- Keyboard, mouse, and clipboard could fail to connect between machines — the QUIC handshake rejected the peer with `invalid peer certificate: BadSignature`. The transport now pins the device's advertised certificate directly instead of running brittle chain validation over a self-signed certificate, which fixes cross-platform (macOS ↔ Windows) handshakes.

### Improved

- **Android: one-tap diagnostics.** One log covering both the app and the protocol core — view it, share it, clear it, or send it to the desktop in a single tap, where it lands next to the desktop's own log. Key events record their mapping and whether injection succeeded.

- **Android: tablet PC mode.** The receiver reports the full display size, so the desktop's cursor maps correctly even inside a freeform window; the on-screen pointer size is adjustable.

### Removed

- In-app update checking and auto-install (the "App Updates" panel and title-bar update badge). This fork does not publish updater artifacts, so installing updates now means grabbing a new installer from Releases manually. The updater plugin, its Tauri config, the `latest.json` manifest job and the signing-key checks were removed from the build and release workflow.

## v0.4.0

### Added

- Update indicator in the title bar: a download icon appears next to "MyKVM" when a newer version is available — click it to open the update panel.

### Fixed

- "Latest version" in Settings now shows the latest released version once a check completes, instead of staying blank when you are already up to date.
- Corrected the clipboard sync description: images are synced too; only file clipboards are unsupported.

## v0.3.4

### Added

- Encrypted QUIC transport for keyboard, mouse, and clipboard traffic (TLS 1.3, pinned to the paired device's certificate).
- In-app updates: check GitHub Releases and install the latest version without leaving MyKVM.
- Clipboard image sync — copy a picture on one machine and paste it on the other (text was already supported).
- Roam across a remote machine's multiple monitors.
- Cross-platform installers for macOS, Windows, and Linux, built automatically on each release.
- Signed macOS builds, so the Accessibility permission survives app updates.

### Improved

- Smoother, more seamless mouse hand-off when crossing between machines and displays.
- Better modifier-key remapping between macOS and Windows.
- Smoother slide-back when MyKVM is not the front window on macOS.
- More reliable LAN discovery and manual peer connection.

### Fixed

- Trackpad two-finger scrolling on the Settings page.
- Faster, more reliable Windows clipboard sync.

## v0.1.0

- Added server/client onboarding and display layout editing.
- Added LAN discovery, manual peer connection, and shared input transport.
- Added text clipboard sync.
- Added English and Simplified Chinese UI strings.
- Added light, dark, and system theme modes.
- Added configurable single-port UDP transport with fallback.
- Added opt-in app performance monitoring.
- Added GitHub Actions CI and tag-based desktop release builds.
