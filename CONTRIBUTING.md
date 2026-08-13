# Contributing to Waku

Thanks for helping improve Waku. Bug reports, focused fixes, tests, and
well-scoped features are welcome.

## Development setup

The debug app currently requires:

- Rust 1.96 or newer
- Bun
- A supported agent CLI when testing a provider integration

macOS is the canonical development platform. Linux and Windows also build:

- **Linux**: install native deps via `.agents/setup` (Wayland/X11, freetype,
  fontconfig, openssl, webkit2gtk, gtk3). `bun run dev` builds
  `target/debug/waku` and launches it directly (no `.app` bundle).
- **Windows**: install the MSVC toolchain and run `bun run dev`, which builds
  `target/debug/waku.exe`.

Some macOS-native surfaces are gated off on Linux/Windows today: the Linux
browser (GPUI/wry handle mismatch), the Computer Use native accessibility
backend (needs AT-SPI on Linux, UI Automation on Windows), and the Sparkle
updater. The Windows browser (WebView2) and Computer Use portable resources
(REPL, Pi extension, skills) build cross-platform; see
[Platform support](../README.md#platform-support) and
[docs/cross-platform.md](docs/cross-platform.md).

Install dependencies and start the development watcher from the repository
root:

```sh
bun install
bun run dev
```

The watcher builds, launches, and rebuilds after source changes. On macOS it
produces a signed `Waku Debug.app`; on Linux and Windows a plain cargo binary.
Keep that watcher running while you work. Do not start a second watcher or
manually relaunch the debug binary. Press `Ctrl-C`, or quit the app, to stop it.

## Making changes

- Before starting work on anything larger than a bug fix, open an issue and
  discuss the proposal first.
- Keep changes focused and follow the existing Rust and GPUI conventions.
- Keep filesystem, process, network, and other blocking work off the UI thread.
  Rendering and row-building paths must read data already held in memory.
- Keep long collections virtualized and per-frame work proportional to visible
  content.
- Make every mouse control keyboard-operable, preserve visible focus, honor
  reduce-motion settings, and do not communicate state with color alone.
- Prefer provider-neutral behavior when a change applies to every agent, while
  preserving provider-native event order and session semantics.
- Add or update tests for behavior that can be verified without the UI.

## Checks

Run the focused checks relevant to your change, then run the full baseline
before opening a pull request:

```sh
cargo fmt --package waku -- --check
cargo check
cargo test
```

For user-visible changes, wait for the watcher to report a successful rebuild
and validate the freshly relaunched app. Include screenshots or a short
recording in the pull request when they make the result easier to review.

## Pull requests

In the pull request description:

- Explain the problem and the chosen solution.
- List the checks you ran.
- Call out known limitations or follow-up work.
- Link the related issue, if one exists.

By contributing, you agree that your contribution will be licensed under the
[GNU General Public License v3.0 only](LICENSE).
