#!/bin/sh
# Builds a minimal Linux distribution of Waku: the cargo binary plus a
# FreeDesktop `.desktop` entry and an install/uninstall helper. This is
# intentionally not an AppImage/deb/rpm — those need heavier tooling
# (appimagetools, dpkg-deb, rpmbuild) and a per-distro release pipeline.
# The macOS .app bundle stays the canonical packaged artifact; on Linux
# the binary + `.desktop` is the smallest correct native distribution.
#
# Usage:
#   scripts/bundle-linux.sh debug      # target/debug/waku
#   scripts/bundle-linux.sh release    # target/release/waku + strip
#
# Install into ~/.local after bundling:
#   scripts/bundle-linux.sh release install
set -eu

profile="${1:-debug}"
action="${2:-bundle}"
cargo_target_dir="${CARGO_TARGET_DIR:-target}"

case "$profile" in
  debug) cargo_profile="debug" ;;
  release) cargo_profile="release" ;;
  *) echo "usage: scripts/bundle-linux.sh [debug|release] [bundle|install|uninstall]" >&2; exit 2 ;;
esac

binary="$cargo_target_dir/$cargo_profile/waku"
computer_use_binary="$cargo_target_dir/$cargo_profile/waku_computer_use_linux"
if [ ! -x "$binary" ] || [ ! -x "$computer_use_binary" ]; then
  echo "[bundle-linux] building $cargo_profile binary..."
  if [ "$profile" = "release" ]; then
    cargo build --release --bin waku --bin waku_js_repl --bin waku_computer_use_linux
  else
    cargo build --bin waku --bin waku_js_repl --bin waku_computer_use_linux
  fi
fi

# The repl helper is a sibling binary on Linux (no .app Resources/).
repl_binary="$cargo_target_dir/$cargo_profile/waku_js_repl"

desktop_entry_name="Waku"
desktop_exec_name="waku"
desktop_icon_name="waku"

write_desktop_entry() {
  cat <<EOF
[Desktop Entry]
Type=Application
Name=Waku
GenericName=Coding Agent Control Plane
Comment=Work with local coding agents in a native desktop app
Exec=$desktop_exec_name %U
Icon=$desktop_icon_name
Terminal=false
Categories=Development;Utility;
StartupWMClass=Waku
EOF
}

case "$action" in
  bundle)
    out="$cargo_target_dir/$cargo_profile/linux-dist"
    mkdir -p "$out/bin"
    cp "$binary" "$out/bin/waku"
    cp "$computer_use_binary" "$out/bin/waku_computer_use_linux"
    [ -x "$repl_binary" ] && cp "$repl_binary" "$out/bin/waku_js_repl" || true
    # Computer Use resources are resolved relative to the executable — see
    # src/computer_use.rs.
    mkdir -p "$out/bin/resources/computer-use" "$out/bin/resources/skills/waku-computer-use"
    cp resources/computer-use/pi-extension.ts "$out/bin/resources/computer-use/pi-extension.ts"
    cp resources/computer-use/SKILL.md "$out/bin/resources/skills/waku-computer-use/SKILL.md"
    write_desktop_entry > "$out/waku.desktop"
    echo "[bundle-linux] wrote $out (bin/waku, waku.desktop)"
    echo "[bundle-linux] install with: scripts/bundle-linux.sh $profile install"
    ;;
  install)
    prefix="${PREFIX:-$HOME/.local}"
    bindir="$prefix/bin"
    appsdir="$prefix/share/applications"
    mkdir -p "$bindir" "$appsdir"
    cp "$binary" "$bindir/waku"
    cp "$computer_use_binary" "$bindir/waku_computer_use_linux"
    chmod 755 "$bindir/waku"
    [ -x "$repl_binary" ] && cp "$repl_binary" "$bindir/waku_js_repl" && chmod 755 "$bindir/waku_js_repl" || true
    mkdir -p "$bindir/resources/computer-use" "$bindir/resources/skills/waku-computer-use"
    cp resources/computer-use/pi-extension.ts "$bindir/resources/computer-use/pi-extension.ts"
    cp resources/computer-use/SKILL.md "$bindir/resources/skills/waku-computer-use/SKILL.md"
    # `$PREFIX` is exported so the .desktop entry's Exec resolves the real
    # install location; without it the entry would point at a bare `waku`
    # that only works if `$bindir` is already on PATH.
    desktop_exec_name="$bindir/waku"
    write_desktop_entry > "$appsdir/waku.desktop"
    echo "[bundle-linux] installed → $bindir/waku, $appsdir/waku.desktop"
    echo "[bundle-linux] note: install a 512x512 PNG named waku.png into $prefix/share/icons/hicolor/512x512/apps/ for the icon."
    ;;
  uninstall)
    prefix="${PREFIX:-$HOME/.local}"
    rm -f "$prefix/bin/waku" "$prefix/bin/waku_js_repl" "$prefix/share/applications/waku.desktop"
    echo "[bundle-linux] uninstalled from $prefix"
    ;;
  *)
    echo "usage: scripts/bundle-linux.sh [debug|release] [bundle|install|uninstall]" >&2
    exit 2
    ;;
esac
