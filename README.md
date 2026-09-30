# Screen Bridge for macOS

Screen Bridge is a macOS app that turns a nearby tablet or computer browser into an extended display. It pairs over the local network and offers Wi-Fi or cable connection modes.

## Download

Download `Screen Bridge_0.1.0_aarch64.dmg` from the [GitHub Releases](../../releases) page. This build is for Apple silicon Macs.

## Current build

- Version: 0.1.0
- Architecture: Apple silicon (`arm64`)
- Minimum macOS version declared by the app: 12.0
- App framework: Tauri 2 with a Rust native backend and web-based interface
- The app uses macOS virtual-display APIs. Audio capture may require macOS 14.2 or later.

## Install

1. Download the DMG from Releases and open it.
2. Drag Screen Bridge to Applications.
3. Open the app and choose a device and connection mode.
4. Follow the on-screen pairing steps.

The app may require macOS privacy permissions for screen recording, audio capture, or local network access, depending on the selected features and macOS version.

## Source availability

The installed app and a packaged DMG were available for this audit, but the original Tauri/Rust source project was not present in the available repositories or app bundle. This repository currently distributes the app only; it does not claim to contain the app's source code or an open-source license. The source project can be added when recovered.

## Feature notes

The original release notes describe QR/PIN pairing, touch input relay, mDNS discovery, and use with iPad, Android, and browser-equipped computers. The installed app exposes device selection, Wi-Fi/cable modes, and quality, resolution, audio, and language settings. The full tablet connection flow was not exercised during this audit, so compatibility and latency claims should be treated as release-note claims rather than independently tested results.
