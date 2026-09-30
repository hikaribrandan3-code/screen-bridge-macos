# Screen Bridge for macOS

Screen Bridge is a native Mac app and a browser-based tablet client for experimenting with an extended display over a local network. Its source combines a Tauri 2 / React interface with a Rust backend that creates a virtual display, captures frames, serves a pairing page, and relays tablet input.

## Stack

- React 18, Vite, and the Tauri 2 JavaScript API
- Rust with Axum WebSockets, mDNS discovery, Core Graphics, and Objective-C bindings
- HTML/JavaScript client served locally to the paired tablet; no tablet app is required
- macOS 12 or later is declared in the app bundle config; Apple Silicon is the currently published build target

## Current status

- Version 0.1.0 source is available in [`app-source/`](app-source/).
- The repository includes Apple Silicon ZIP and DMG artifacts. The app is ad-hoc signed and not notarized, so macOS may show a first-open warning.
- `CGVirtualDisplay` is a private macOS API resolved at runtime; macOS updates may affect it.
- Pairing, display quality, touch input, cable networking, audio behavior, and compatibility across tablet/browser combinations still need hands-on release verification. Treat feature descriptions as implementation scope, not a compatibility guarantee.

## Build

On a Mac with Rust and Node.js installed:

```sh
cd app-source
npm install
npm run tauri dev
```

To create a release bundle:

```sh
npm run tauri build
```

The generated `.app` and `.dmg` are written under `app-source/src-tauri/target/release/bundle/`.

## Download

- [Apple Silicon ZIP](https://screen-bridge-macos.vercel.app/Screen-Bridge-0.1.0-apple-silicon.zip)
- [DMG artifact in this repository](https://github.com/hikaribrandan3-code/screen-bridge-macos/blob/main/Screen%20Bridge_0.1.0_aarch64.dmg)

This is a free, open-source project. Source is licensed under MIT; third-party components retain their own licenses.
