# Cross-platform support

Waku's first shipped platform was macOS. Linux and Windows now build and run
the app shell; a few macOS-native surfaces are gated off until their
platform-native backends land. This document is the map of what works, what is
stubbed, and where each missing piece slots in.

## Build status

| Target | `cargo check` | `cargo test` | runtime | packaging |
| --- | :---: | :---: | :---: | :---: |
| `x86_64-apple-darwin` | ✅ | ✅ | ✅ | `.app` (`scripts/bundle.sh`) |
| `x86_64-unknown-linux-gnu` | ✅ | ✅ (1) | ✅ (2) | `.desktop` + binary (`scripts/bundle-linux.sh`) |
| `x86_64-pc-windows-gnu` | ✅ (3) | n/a (3) | unverified (4) | portable `.exe` (`scripts/bundle-windows.sh`) |
| `x86_64-pc-windows-msvc` | unverified (4) | unverified (4) | unverified (4) | portable `.exe` (`scripts/bundle-windows.sh`) |

1. One `checkpoint` test needs `git merge-tree --merge-base` (git ≥ 2.42).
   Older git (Debian 12 ships 2.39) skips that test; the failure is the
   environment's git version, not Waku.
2. Runtime validation needs a real Linux desktop (Wayland/X11 + GPU). The
   development orb is headless, so runtime checks happen on developer
   machines, not in CI here.
3. The `x86_64-pc-windows-gnu` target is cross-checked from Linux with
   `mingw-w64`. This proves the Windows cfg arms type-check against wry's
   WebView2 backend; it does not prove the WebView2 runtime works (that
   needs a Windows host with Edge installed).
4. MSVC is the canonical Windows target but cannot be built from this orb
   (no MSVC toolchain). A Windows host with `rustup target
   x86_64-pc-windows-msvc` should compile the same cfg arms.

## Platform seam

All platform-specific code lives behind `cfg(target_os = …)` arms in
`src/platform.rs`, `src/browser.rs`, `src/updater.rs`, `src/i18n.rs`,
`src/usage.rs`, and `src/computer_use.rs`. The non-macOS arms are not fake —
they either implement the behavior with a native equivalent (Linux `gio trash`,
`xdg-open`, `notify-send`, `gsettings`; Windows `powershell` + WinRT/UIAutomation
probes) or return a clear "not available" that the UI surfaces as a message.

## Gated surfaces and their native backends

### Browser (`src/browser.rs::mod host`)

The macOS host is a WKWebView attached as a child `NSView`, with a KVO
first-responder observer and a snapshot/overlay dance so GPUI overlays layer
above native page pixels.

- **macOS**: full surface — WKWebView child view, KVO focus observer,
  snapshot/frozen-overlay for menus, download→Finder reveal, Safari UA.
- **Windows**: WebView2 child window via wry's `build_as_child` against the
  GPUI window's HWND. URL/load/title/devtools/navigation all go through
  wry's portable API. Edge UA. No KVO focus observer (Windows focus is
  window-scoped, not responder-chain-scoped) — `native_focus_within`
  returns `false` and GPUI's own focus drives the surface. No
  snapshot/overlay: the child HWND is hidden while a menu is open. `stop`
  is a no-op (wry's portable API doesn't expose WebView2's Stop yet).
- **Linux/X11**: native WebKitGTK child surface via wry. GPUI exposes an Xcb
  handle while wry requires Xlib, so Waku adapts the X11 window id before
  calling `build_as_child`. Bounds, visibility, navigation, downloads and
  title/load callbacks use the same path as Windows.
- **Linux/Wayland**: still gated. wry requires a GTK container for a
  Wayland-compatible host, while GPUI currently exposes no GTK container or
  foreign-subsurface seam. Waku reports a clear host error instead of
  pretending the X11 adapter works on Wayland.

wry is a dependency on macOS, Windows and Linux. Linux additionally needs
GTK 3 and WebKitGTK 4.1 development/runtime packages (`gtk3-devel` and
`webkit2gtk4.1-devel` on Fedora). The macOS-specific focus/snapshot code stays
`cfg(target_os = "macos")` inside the host module; Windows and Linux use a
plain native child webview without the overlay refinement.

### Computer Use (`src/computer_use.rs`, `src/driver/computer_use.rs`)

The macOS backend is a signed Swift helper (`resources/computer-use/*.swift`)
that drives Accessibility — see
[`sky-macos-accessibility-reverse-engineering.md`](sky-macos-accessibility-reverse-engineering.md).

**Portable resources** — the JavaScript REPL (`waku_js_repl`), the Pi
extension (`pi-extension.ts`), and the skill (`SKILL.md`) — are resolved
cross-platform by `js_repl_server_path`, `pi_extension_path`, and
`skill_root_path`. On macOS they live inside the `.app` bundle's
`Contents/Resources/`; on Linux and Windows they ship in a `resources/`
directory next to the executable (the bundle scripts copy them there).

**Native accessibility backend** — `probe_permissions` and
`mcp_server_command` — use the platform backend. `@oai/sky` itself ships
separate backends per OS:

- **Linux**: Waku uses AT-SPI / AT-SPI2 for the accessibility tree and the
  XDG Screenshot/RemoteDesktop portals for Wayland-safe capture and input. The
  backend is shipped as the Rust `waku_computer_use_linux` helper; it never
  shells out to external scripting or `xdotool`.
- **Windows**: UI Automation, GDI desktop capture, and `SendInput`, shipped
  by the Rust `waku_computer_use_windows` helper. The helper is the Windows
  equivalent of the macOS Swift and Linux portal backends.

These are entirely different native APIs, not a port of the Swift helper.
The MCP/`sky` contract is shared, while target resolution, capture, input,
permissions, and stale-element validation stay native to each OS. A Windows
build is only advertised as Computer Use-capable when the native sibling
helper is present; it never silently falls back to another platform.

### Updater (`src/updater.rs`)

macOS uses Sparkle, embedded by `scripts/bundle.sh`. The non-macOS `Updater`
stub returns `None` from `init()`, so the menu item is omitted and the
sidebar shows no update state.

- **Windows**: WinSparkle consumes the same appcast format Sparkle produces.
  `scripts/appcast.ts` already generates it. Embedding WinSparkle needs the
  Windows SDK at bundle time.
- **Linux**: no cross-distro auto-updater standard. AppImage delta updates or
  a "check for updates → open browser" fallback are the realistic options.

### Packaging

- **macOS**: `scripts/bundle.sh` (codesign, Sparkle, Swift helper, `.app`).
- **Linux**: `scripts/bundle-linux.sh` (binary + `.desktop` + install helper).
  Not an AppImage/deb/rpm — those need per-distro tooling and a release
  pipeline.
- **Windows**: `scripts/bundle-windows.sh` stages the `.exe` files and
  documents the `signtool` + WiX/MSIX steps that need a Windows host.

## Development environment

`.agents/setup` installs Rust and the Linux native deps a fresh orb needs.
Re-running it is safe. On macOS and Windows it is a no-op for the system
packages (only the Rust toolchain step runs).
