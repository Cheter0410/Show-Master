#!/bin/bash
# Show Master on the Mac: update this checkout, build the DMG and install the app.
#
#   mkdir -p ~/Desktop/dev
#   git clone https://github.com/Cheter0410/Show-Master.git ~/Desktop/dev/show-master
#   cd ~/Desktop/dev/show-master
#   bash install-mac.sh
#
# Run it again later to update (it pulls the newest version first). Missing
# tools (Xcode command line tools, Rust) are installed. The result:
#   dist/ShowMaster-x.y.z.dmg   installer (this Mac's type; UNIVERSAL=1 for Intel + Apple Silicon)
#   /Applications/ShowMaster.app   installed on this Mac (skip with NO_INSTALL=1)
set -euo pipefail
cd "$(dirname "$0")"
[ "$(uname -s)" = "Darwin" ] || { echo "This script is for macOS (Linux: ./install.sh)."; exit 1; }

say() { printf '\n\033[1m== %s\033[0m\n' "$*"; }

if ! xcode-select -p >/dev/null 2>&1; then
  xcode-select --install || true
  echo
  echo ">>> In the window that opened, click 'Install' and wait until it is done."
  echo ">>> Then run this again:  bash install-mac.sh"
  exit 1
fi

if [ -z "${SM_PULLED:-}" ] && [ -d .git ]; then
  if git diff --quiet && git diff --cached --quiet; then
    say "Fetching the newest version"
    git pull --ff-only || echo "(could not update - building the version that is here)"
    # Run the freshly pulled copy of this script (bash would go on reading
    # the old one).
    SM_PULLED=1 exec bash "$(pwd)/$(basename "$0")" "$@"
  else
    echo "(local changes in $(pwd) - not updating; 'git status' shows them)"
  fi
fi

if [ ! -x "$HOME/.cargo/bin/rustup" ] && ! command -v rustup >/dev/null 2>&1; then
  say "Installing Rust"
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal
fi
# shellcheck disable=SC1091
[ -f "$HOME/.cargo/env" ] && . "$HOME/.cargo/env"

# This Mac's architecture only (one compile). UNIVERSAL=1 builds the DMG for
# Intel and Apple Silicon Macs (for publishing a release).
if [ -n "${UNIVERSAL:-}" ]; then
  export ARCHS="arm64 x86_64"
elif [ "$(sysctl -n hw.optional.arm64 2>/dev/null || echo 0)" = "1" ]; then
  export ARCHS="arm64"
else
  export ARCHS="x86_64"
fi
say "Building the DMG for $ARCHS only (takes a few minutes the first time)"
./packaging/macos/build-dmg.sh
DMG=$(ls -t dist/ShowMaster-*.dmg | head -n 1)

# Offer to upload the DMG to the release the in-app updater looks at
# (only when the GitHub command line is set up; PUBLISH=0 never asks).
if command -v gh >/dev/null 2>&1 || [ -n "${PUBLISH:-}" ]; then
  ./packaging/publish.sh || true
fi

if [ -n "${NO_INSTALL:-}" ]; then
  open dist
  say "Done: $DMG"
  exit 0
fi

if pgrep -x showmaster >/dev/null 2>&1; then
  say "Show Master is running"
  echo "Not replacing the app while it runs (a show may be going on)."
  echo "Quit it (Quit in the window, Cmd+Q or the menu bar icon), then run this again -"
  echo "or install from the DMG: $DMG"
  open dist
  exit 0
fi

say "Installing into /Applications"
MNT=$(mktemp -d)
hdiutil attach -nobrowse -readonly -mountpoint "$MNT" "$DMG" >/dev/null
trap 'hdiutil detach "$MNT" -quiet >/dev/null 2>&1 || true; rmdir "$MNT" 2>/dev/null || true' EXIT
rm -rf /Applications/ShowMaster.app
ditto "$MNT/ShowMaster.app" /Applications/ShowMaster.app
xattr -dr com.apple.quarantine /Applications/ShowMaster.app 2>/dev/null || true

say "Done"
echo "Show Master is in Programme / Applications (start it from Launchpad or Spotlight)."
echo "The installer for other Macs: $DMG"
echo "On a phone or tablet in the same network: showmaster.local"
