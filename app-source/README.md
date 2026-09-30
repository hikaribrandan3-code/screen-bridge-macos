# Screen Bridge source

This folder contains the macOS app and browser client source for Screen Bridge 0.1.0.

## Stack

- React 18 and Vite for the Mac interface
- Tauri 2 for the macOS shell and Rust command bridge
- Rust/Axum for the local WebSocket server, device discovery, frame transport, and input relay
- HTML/JavaScript pages for pairing and the tablet-side browser client
- macOS Core Graphics and Objective-C bindings for virtual-display and screen-capture operations

## Build on macOS

Install Rust and Node.js, then run:

```sh
npm install
npm run tauri dev
```

To package the app:

```sh
npm run tauri build
```

Build output is placed in `src-tauri/target/release/bundle/`.

## Implementation status

The backend resolves Apple's private `CGVirtualDisplay` API at runtime, captures frames, and serves the tablet page over a local network connection. The tablet page contains pairing, display, audio, and input handling code. The complete pairing and display experience has not been independently verified across different Macs, tablets, browsers, or network setups. Treat compatibility and performance as work in progress.

The app declares macOS 12 or later. Some audio APIs may need macOS 14.2 or later. The currently published build is Apple Silicon only and is ad-hoc signed, not notarized.
