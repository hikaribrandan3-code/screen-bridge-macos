# Screen Bridge development notes — 30 September 2026

## Project origin and initial development

Screen Bridge began as a personal experiment in using a tablet as a Mac display with a browser client. AI coding tools assisted the original Tauri/React interface and Rust display, capture, pairing, audio, and input implementation. The September release pass did not create those systems from scratch.

## Hands-on usage

The creator has used Screen Bridge successfully on their Mac. It worked, with noticeable lag and incomplete reliability. No universal device compatibility or low-latency performance is claimed.

## Source audit: 30 September 2026

The app reported a session before confirming that its local HTTP server could bind. A virtual-display creation failure was silently converted to `None`, so the Mac UI could remain waiting. The token was in the QR URL while responses lacked no-referrer and no-store headers. The code uses private `CGVirtualDisplay` Objective-C objects with an unsafe `Send` implementation and a CoreAudio raw callback pointer. The display object's ownership is tied to `DisplaySlot`; the callback context is boxed and held until IOProc shutdown. These lifetime patterns have a rationale, but Apple's private API provides no documented future compatibility or threading guarantee.

## Improvements made

- Bound the local server before reporting a connected session; forwarded runtime server and virtual-display errors to the Mac UI.
- Added no-referrer, no-store, and nosniff HTTP response headers for pairing pages and a referrer policy in the tablet page.
- Added null/result checks to virtual-display setup, clarified Objective-C/CoreAudio ownership comments, and removed an unused helper.
- Updated the JavaScript lockfile within existing version ranges; remaining dev-only Vite/esbuild advisories require a major toolchain upgrade and are recorded as a limitation.
- Corrected source and product documentation to label the project experimental and explain permissions, transport, and compatibility.

## Verification performed

The Rust source passed `cargo check`; the React/Vite interface passed `npm run build`; `npm audit --omit=dev` found zero production JavaScript advisories. The 0.1.1 Tauri app and DMG compiled. Strict macOS verification rejected the app signature initially emitted in the DMG, so I ad-hoc signed the app bundle, verified that bundle, and staged it as a ZIP. The DMG is not distributed. The Mac app still requires a fresh creator smoke test on real display and tablet hardware before the updated binary is published. Existing Rust hardware tests request real display and audio access and were not run for this source pass; they are now ignored by default. `cargo fmt --check` could not run because the local rustfmt component was absent.

## Known limitations and current status

The HTTP/WebSocket stream is unencrypted and intended only for a trusted local network. QR URLs and PINs must be kept private. macOS may change the private display API; audio needs a newer OS than the declared app minimum. Latency and compatibility vary by hardware and network. This is an experimental but credible prototype with public source, not a production-ready general display solution.
