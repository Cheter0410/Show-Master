#!/bin/bash
# Show Master on the Mac: update this checkout, build the DMG and install the app.
#
#   mkdir -p ~/Desktop/dev
#   git clone https://github.com/Cheter0410/Show-Master.git ~/Desktop/dev/show-master
#   cd ~/Desktop/dev/show-master
#   bash install-mac.sh
#
# Run it again later to update. Only what is needed is done: an unchanged DMG
# is not built again, an unchanged app not installed again. Missing tools
# (Xcode command line tools, Rust) are installed.
#   dist/ShowMaster-x.y.z.dmg      installer (this Mac's type; UNIVERSAL=1 for Intel + Apple Silicon)
#   /Applications/Stagehand/ShowMaster.app   installed on this Mac (skip with NO_INSTALL=1)
set -euo pipefail
cd "$(dirname "$0")"
# shellcheck source=scripts/ui.sh
. scripts/ui.sh
[ "$(uname -s)" = "Darwin" ] || ui_fail "Dieses Skript ist für macOS (Linux: ./install.sh)."
[ -n "${SM_PULLED:-}" ] || ui_header "Show Master" "Installieren / Aktualisieren"

if ! xcode-select -p >/dev/null 2>&1; then
  xcode-select --install || true
  ui_fail "Im Fenster, das sich geöffnet hat, auf 'Installieren' klicken und warten. Dann nochmal: bash install-mac.sh"
fi

if [ -z "${SM_PULLED:-}" ] && [ -d .git ]; then
  if git diff --quiet && git diff --cached --quiet; then
    git fetch -q 2>/dev/null || true
    if [ -n "$(git rev-list HEAD..@{u} 2>/dev/null)" ]; then
      ui_run "Neuesten Stand holen" git pull -q --ff-only
      SM_PULLED=1 exec bash "$(pwd)/$(basename "$0")" "$@"
    fi
    ui_ok "Quellcode ist aktuell"
  else
    ui_warn "Lokale Änderungen – baue den Stand, der hier liegt"
  fi
else
  ui_ok "Quellcode ist aktuell"
fi

if [ ! -x "$HOME/.cargo/bin/rustup" ] && ! command -v rustup >/dev/null 2>&1; then
  ui_run "Rust installieren (einmalig)" sh -c "curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal"
fi
# shellcheck disable=SC1091
[ -f "$HOME/.cargo/env" ] && . "$HOME/.cargo/env"

ui_section "Bauen"
./packaging/macos/build-dmg.sh
V=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)
DMG="dist/ShowMaster-$V.dmg"

ui_section "Installieren"
if [ -n "${NO_INSTALL:-}" ]; then
  ui_skip "Installieren" "NO_INSTALL gesetzt"
  ui_ok "Installer: $DMG"
  ui_done
  exit 0
fi
# The DMG this app came from (kept next to the build outputs).
STAMP=dist/.made/installed-app
# All show tools share one folder (as Stagehand installs them).
APPDIR=/Applications/Stagehand
WANT=$(cat "dist/.made/$(basename "$DMG")" 2>/dev/null || echo none)
if [ -d "$APPDIR/ShowMaster.app" ] && [ "$(cat "$STAMP" 2>/dev/null)" = "$WANT" ]; then
  ui_skip "In Programme/Stagehand" "Show Master $V ist schon installiert"
elif pgrep -x showmaster >/dev/null 2>&1; then
  ui_warn "Show Master läuft gerade – nicht ersetzt (es könnte eine Show laufen)."
  ui_info "Beenden (Quit, Cmd+Q oder Menüleisten-Symbol), dann nochmal: bash install-mac.sh"
else
  put() {
    MNT=$(mktemp -d)
    hdiutil attach -nobrowse -readonly -mountpoint "$MNT" "$DMG" >/dev/null
    mkdir -p "$APPDIR"
    rm -rf "$APPDIR/ShowMaster.app" "/Applications/ShowMaster.app"
    ditto "$MNT/ShowMaster.app" "$APPDIR/ShowMaster.app"
    hdiutil detach "$MNT" -quiet >/dev/null 2>&1 || true
    rmdir "$MNT" 2>/dev/null || true
    xattr -dr com.apple.quarantine "$APPDIR/ShowMaster.app" 2>/dev/null || true
  }
  ui_run "In Programme/Stagehand" put
  printf '%s' "$WANT" > "$STAMP"
fi

ui_section "Fertig"
ui_ok "Show Master ist in Programme › Stagehand (Launchpad, Spotlight)"
ui_ok "Am Handy / Tablet im selben Netz:  showmaster.local"
ui_done
