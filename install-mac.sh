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
# shellcheck source=scripts/setup.sh
. scripts/setup.sh
[ "$(uname -s)" = "Darwin" ] || ui_fail "Dieses Skript ist für macOS (Linux: ./install.sh)."
setup_no_root
[ -n "${SM_PULLED:-}" ] || ui_header "Show Master" "Installieren / Aktualisieren"

if ! xcode-select -p >/dev/null 2>&1; then
  xcode-select --install || true
  ui_fail "Im Fenster, das sich geöffnet hat, auf 'Installieren' klicken und warten. Dann nochmal: bash install-mac.sh"
fi

if [ -z "${SM_PULLED:-}" ]; then
  if setup_pull; then SM_PULLED=1 exec bash "$(pwd)/$(basename "$0")" "$@"; fi
fi

setup_rust

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
APPDIR=$(mac_app_dir)
WANT=$(cat "dist/.made/$(basename "$DMG")" 2>/dev/null || echo none)
if [ -d "$APPDIR/ShowMaster.app" ] && [ "$(cat "$STAMP" 2>/dev/null)" = "$WANT" ]; then
  ui_skip "In $APPDIR" "Show Master $V ist schon installiert"
elif pgrep -x showmaster >/dev/null 2>&1; then
  ui_warn "Show Master läuft gerade – nicht ersetzt (es könnte eine Show laufen)."
  ui_info "Beenden (Quit, Cmd+Q oder Menüleisten-Symbol), dann nochmal: bash install-mac.sh"
else
  ui_run "In $APPDIR" mac_app_put "$DMG" ShowMaster.app "$APPDIR"
  printf '%s' "$WANT" > "$STAMP"
fi

ui_section "Fertig"
ui_ok "Show Master ist in Programme › Stagehand (Launchpad, Spotlight)"
ui_ok "Am Handy / Tablet im selben Netz:  showmaster.local"
ui_done
