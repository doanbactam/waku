# Waku

Waku is a fast, native desktop app for working with local coding agents. It is
built in Rust with [GPUI](https://github.com/zed-industries/zed/tree/main/crates/gpui)
and keeps projects, sessions, transcripts on your machine.

[Download Waku](https://waku.sh)

## Supported agents

Waku works with:

- [Amp](https://ampcode.com/)
- Claude Code
- Codex CLI
- Cursor CLI
- Grok Build
- OpenCode
- Pi

Install and authenticate at least one supported agent CLI before starting Waku.
Waku detects available CLIs automatically and uses each provider's native
structured protocol and session continuity.

## Highlights

- Keep projects and independent agent sessions in one native app.
- Switch models, reasoning effort, and access modes from a shared interface.
- Queue or steer follow-up messages while an agent is working.
- Rewind Git-backed tasks with conversation-aware checkpoints.
- Store app state locally, with no Waku account or remote service required.

## Development

Development requires [Rust 1.96 or newer](https://www.rust-lang.org/tools/install)
and [Bun](https://bun.sh/). macOS is the canonical development platform; Linux
and Windows also build and run, with some macOS-only surfaces gated off (see
[Platform support](#platform-support)).

```sh
bun install
bun run dev
```

On macOS the watcher builds and signs a `Waku Debug.app` bundle via
`scripts/bundle.sh`. On Linux and Windows it builds a plain cargo binary
(`target/debug/waku` or `waku.exe`) and launches it directly.

See [CONTRIBUTING.md](CONTRIBUTING.md) for the development workflow and checks.
Release maintainers should also read [RELEASING.md](RELEASING.md).

Debug builds use the app name `Waku Debug`, bundle identifier `sh.waku.dev`,
and keep `app.db`, `state.json`, and `settings.json` in the checkout's ignored
`temp/` directory. Release builds use `Waku`, bundle identifier `sh.waku`, keep
app-managed data in the `Waku` Application Support directory, and read the
user-editable settings file from `~/.waku/settings.json`.

## Platform support

Waku's native surfaces have different maturity per platform:

| Surface | macOS | Linux | Windows |
| --- | :---: | :---: | :---: |
| App shell (window, sidebar, transcript, composer) | ✅ | ✅ builds, runs | ✅ builds |
| Browser surface (right panel) | ✅ | ✅ X11 / gated on Wayland (1) | ✅ builds (WebView2) |
| Computer Use | ✅ | ✅ (AT-SPI + XDG portals) | ✅ (UI Automation) |
| In-app updater (Sparkle) | ✅ | not yet | not yet (WinSparkle planned) |
| Packaging | `.app` | `.desktop` + binary | portable `.exe` (MSI via WiX) |

`cargo check` and `cargo test` pass on Linux; the Windows cfg arms are
cross-checked from Linux with `mingw-w64` (`x86_64-pc-windows-gnu`). Runtime
validation on Linux and Windows requires a real desktop (the development orb
is headless).

1. Linux X11 uses a native WebKitGTK child webview through an XCB→Xlib window
   adapter. Wayland remains gated until GPUI exposes a GTK container or
   foreign-subsurface host. See [docs/cross-platform.md](docs/cross-platform.md)
   for the required native packages and details.

## License

Waku is licensed under the [GNU General Public License v3.0 only](LICENSE).
