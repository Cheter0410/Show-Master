#!/bin/bash
# One-stop Mac build: installs what is missing (Xcode command line tools, Rust),
# then builds dist/ShowMaster-x.y.z.dmg and shows it in the Finder.
#   bash packaging/macos/setup-and-build.sh
# Signing / notarization: set SIGN_IDENTITY (and NOTARY_PROFILE), see build-dmg.sh.
set -euo pipefail
cd "$(dirname "$0")/../.."

if ! xcode-select -p >/dev/null 2>&1; then
  xcode-select --install || true
  echo
  echo ">>> In the window that opened, click 'Install' and wait until it is done."
  echo ">>> Then run this command again:  bash packaging/macos/setup-and-build.sh"
  exit 1
fi

if [ ! -x "$HOME/.cargo/bin/rustup" ] && ! command -v rustup >/dev/null 2>&1; then
  echo ">>> Installing Rust..."
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
fi
# shellcheck disable=SC1091
[ -f "$HOME/.cargo/env" ] && . "$HOME/.cargo/env"

./packaging/macos/build-dmg.sh
open dist
echo
echo ">>> Done: the installer is dist/ShowMaster-*.dmg (runs on Intel and Apple Silicon Macs)."
