#!/bin/sh
# Windows distribution helper for Waku.
#
# Unlike macOS (codesign + Sparkle + .app) and Linux (binary + .desktop),
# Windows packaging requires the Windows SDK and a WiX toolchain (or
# msix packaging) that this repository does not vendor. This script
# performs the portable part: build the cargo binaries and stage them
# into a clean directory suitable for signing with `signtool` and
# wrapping with WiX/MSIX on a Windows host.
#
# Run on Windows under Git Bash, WSL (cross-compile), or any sh:
#   scripts/bundle-windows.sh debug
#   scripts/bundle-windows.sh release
set -eu

profile="${1:-debug}"
cargo_target_dir="${CARGO_TARGET_DIR:-target}"

case "$profile" in
  debug) cargo_profile="debug" ;;
  release) cargo_profile="release" ;;
  *) echo "usage: scripts/bundle-windows.sh [debug|release]" >&2; exit 2 ;;
esac

# When invoked from a Unix shell but targeting Windows, honor an explicit
# `--target` if set; otherwise a native Windows shell already produces
# *.exe from a plain `cargo build`.
windows_target="${WIN_CARGO_TARGET:-}"
if [ -n "$windows_target" ]; then
  cargo build --release --bin waku --bin waku_js_repl --target "$windows_target" 2>/dev/null || \
    cargo build --bin waku --bin waku_js_repl --target "$windows_target"
  bin_dir="$cargo_target_dir/$windows_target/$cargo_profile"
else
  if [ "$profile" = "release" ]; then
    cargo build --release --bin waku --bin waku_js_repl 2>/dev/null || true
  else
    cargo build --bin waku --bin waku_js_repl 2>/dev/null || true
  fi
  bin_dir="$cargo_target_dir/$cargo_profile"
fi

out="$cargo_target_dir/$cargo_profile/windows-dist"
mkdir -p "$out"
for name in waku waku_js_repl; do
  for ext in exe ""; do
    src="$bin_dir/$name.$ext"
    [ -f "$src" ] && cp "$src" "$out/" && break
  done
done

# Computer Use portable resources (the native accessibility backend is
# macOS-only, but the REPL, Pi extension, and skill are portable and
# resolved relative to the executable — see src/computer_use.rs).
mkdir -p "$out/resources/computer-use" "$out/resources/skills/waku-computer-use"
cp resources/computer-use/pi-extension.ts "$out/resources/computer-use/pi-extension.ts"
cp resources/computer-use/SKILL.md "$out/resources/skills/waku-computer-use/SKILL.md"

cat > "$out/README.txt" <<'EOF'
Waku (Windows portable)
=======================

This directory contains the Waku binaries built by Cargo. To produce a
proper installer:

1. Sign the executables with signtool (Windows SDK):
     signtool sign /fd SHA256 /a /tr http://timestamp.digicert.com /td SHA256 waku.exe waku_js_repl.exe

2. Build an MSI or MSIX with WiX v4+ or the MSIX packaging tool. The
   AppIdentity should be "sh.waku" to match the macOS bundle id.

3. For auto-update on Windows, ship an appcast.xml compatible with
   WinSparkle and embed WinSparkle (https://winsparkle.org). The
   macOS appcast at scripts/appcast.ts already produces the right
   format — WinSparkle consumes the same sparkle:xmlNamespace.
EOF

echo "[bundle-windows] staged $out"
echo "[bundle-windows] follow $out/README.txt to sign and wrap into an installer."
