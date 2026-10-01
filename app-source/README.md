# Screen Bridge source

This folder contains the macOS app and browser client source for Screen Bridge 0.1.1. It is an experimental personal project.

## Stack

- React 18 and Vite for the Mac interface
- Tauri 2 for the macOS shell and Rust command bridge
- Rust/Axum for the local WebSocket server, device discovery, frame transport, and input relay
- HTML/JavaScript pages for pairing and the tablet-side browser client
- macOS Core Graphics and Objective-C bindings for virtual-display and screen-capture operations

## Build on macOS

Install Rust, Node.js, and Xcode Command Line Tools on a Mac, then run:

```sh
npm ci
npm run tauri dev
```

To package the app:

```sh
npm run tauri build
```

Build output is placed in `src-tauri/target/release/bundle/`.

## Implementation status

The backend resolves Apple's private `CGVirtualDisplay` API at runtime, captures frames, and serves the tablet page over local HTTP/WebSockets on port 47788. Pairing uses a short one-use PIN or a token embedded in the QR URL. The tablet page contains display, audio, and input code. Server bind and virtual-display errors are reported in the Mac UI. The complete experience has not been verified across different Macs, tablets, browsers, or networks. Expect lag and occasional failure.

The app declares macOS 12 or later. Audio capture needs macOS 14.2 or later. Screen Recording, Local Network, and optional System Audio Recording access may be needed. Use only on a trusted network: the local transport is not encrypted and a paired browser can send input to the Mac. The currently public build is Apple Silicon 0.1.0, ad-hoc signed and unnotarized; 0.1.1 is staged for smoke testing. For source audit details see [development notes](../DEVELOPMENT_NOTES.md).
