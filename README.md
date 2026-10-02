# Screen Bridge for macOS — experimental

Screen Bridge is a personal experiment in using a tablet browser as an extended display for a Mac over a local network. I built it to explore the full path from a native virtual display through capture, pairing, streaming, and input. I have used it successfully, but it has noticeable lag and is not fully reliable. Treat it as a working prototype, not a replacement for a dedicated display product.

## Stack

- React 18, Vite, and the Tauri 2 JavaScript API for the Mac interface
- Rust/Axum HTTP and WebSockets for pairing, JPEG frames, audio, and input; mDNS for local discovery
- Core Graphics capture, private `CGVirtualDisplay` through Objective-C runtime bindings, and a CoreAudio process tap for optional audio
- HTML/JavaScript tablet client served from the Mac; no tablet app or cloud backend

The Mac app owns a session. Its server listens on port 47788 on local network interfaces, checks a pairing token for display and audio streams, and creates the virtual display after the tablet reports its size. A six-digit one-use PIN is available for devices that cannot scan the QR code. The browser then receives JPEG frames over WebSocket and sends input events back. The Mac process holds the Objective-C display objects and CoreAudio callback context for the session lifetime.

## Current status

- Version 0.1.1 source is in [`app-source/`](app-source/). The older 0.1.0 binaries remain as historical artifacts. The 0.1.1 build is staged for creator smoke testing before public release.
- **Experimental:** `CGVirtualDisplay` is private and unsupported by Apple. It may stop working after a macOS update. Audio capture requires macOS 14.2 or later even though the app bundle declares macOS 12 or later.
- Latency, touch/input behavior, audio routing, cable networking, and compatibility vary by Mac, network, tablet, and browser. Screen Recording, Local Network, and optional System Audio Recording permissions may be requested.
- The app is ad-hoc signed and not notarized, so macOS may show a first-open warning.
- The new 0.1.1 smoke-test artifact is a verified, ad-hoc signed app ZIP. The DMG emitted by the current Tauri build did not contain a fully sealed app signature, so that DMG is not distributed.

## Build

On an Apple Silicon Mac with Rust, Node.js, Xcode Command Line Tools, and the required macOS permissions:

```sh
cd app-source
npm ci
npm run tauri dev
```

To create a release bundle:

```sh
npm run tauri build
```

The generated `.app` and `.dmg` are written under `app-source/src-tauri/target/release/bundle/`. For a distributable app, ad-hoc sign and verify the `.app`, then package that verified bundle as a ZIP. The current Tauri DMG output is not used because its contained signature did not pass strict verification. This project has no automated compatibility matrix. Hardware end-to-end Rust tests request real screen/audio access and are marked ignored for routine test runs.

## Download

- [Previous 0.1.0 Apple Silicon ZIP](https://screen-bridge-macos.vercel.app/Screen-Bridge-0.1.0-apple-silicon.zip)
- [Previous 0.1.0 DMG artifact](https://github.com/hikaribrandan3-code/screen-bridge-macos/blob/main/Screen%20Bridge_0.1.0_aarch64.dmg)

## Privacy and trust boundary

Video, audio, and input move between your Mac and the paired browser across local HTTP/WebSockets; the app has no cloud relay. The transport is **not encrypted**. Anyone on the same network who obtains the QR URL or PIN could connect during that session. Use only a trusted local network, do not share the pairing URL or display sensitive content, and disconnect when done. The server sets no-store and no-referrer headers to reduce token exposure, but a browser may still retain the URL in its history. The app's input relay can control your Mac while paired.

This is free source under the MIT License; third-party components keep their own licenses. AI coding tools assisted development. I defined the product behavior, tested the prototype on my Mac, reviewed the source, and documented limitations and changes in [development notes](DEVELOPMENT_NOTES.md).

## Portfolio evidence

[Mac app suite case study](https://hikari-brandan.vercel.app/projects/macos-app-suite) documents the product story and current limits.
