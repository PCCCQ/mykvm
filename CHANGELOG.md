# Changelog

This file feeds the GitHub Release notes. Keep entries user-facing: describe what
changed for someone *using* MyKVM, not the internal/CI plumbing. The release
workflow publishes whatever is under `## [Unreleased]`, so move those entries
under a version heading when you cut a release (or just leave them — the next
release will reuse them).

## [Unreleased]

### Added

- **Linux input sharing (X11).** Linux machines can now both control other
  machines (edge-crossing capture via pointer/keyboard grabs, with the same
  edge-snap, hotkey switch and clipboard follow as Windows/macOS) and be
  controlled (XTEST injection of mouse, keys, wheel and side buttons, plus
  ReleaseAll so a session teardown never leaves stuck keys). Requires an
  X11/Xorg session; Wayland-native sessions show a clear error instead of
  silently doing nothing.

### Fixed

- Keyboard, mouse, and clipboard could fail to connect between machines — the QUIC handshake rejected the peer with `invalid peer certificate: BadSignature`. The transport now pins the device's advertised certificate directly instead of running brittle chain validation over a self-signed certificate, which fixes cross-platform (macOS ↔ Windows) handshakes.

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
