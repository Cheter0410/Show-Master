#!/bin/bash
# Builds ShowMaster-x.y.z.dmg (universal: Apple Silicon + Intel) on a Mac.
# Needs: Xcode command line tools, Rust (rustup) and Node 22.
#   ./packaging/macos/build-dmg.sh
# The result is in dist/. Used by the `build` workflow and for building by hand.
set -euo pipefail
cd "$(dirname "$0")/../.."
[ "$(uname -s)" = "Darwin" ] || { echo "This script must run on macOS."; exit 1; }
export MACOSX_DEPLOYMENT_TARGET="${MACOSX_DEPLOYMENT_TARGET:-11.0}"

# Prerequisites: say exactly what is missing and how to get it.
missing=0
if ! xcode-select -p >/dev/null 2>&1; then
  echo "Missing: Xcode command line tools.  Install:  xcode-select --install"; missing=1
fi
if ! command -v cargo >/dev/null 2>&1 || ! command -v rustup >/dev/null 2>&1; then
  if [ -x "$HOME/.cargo/bin/cargo" ]; then
    export PATH="$HOME/.cargo/bin:$PATH"
  else
    echo "Missing: Rust.  Install:  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh"
    echo "         then open a new terminal window."; missing=1
  fi
fi
if ! command -v npm >/dev/null 2>&1; then
  for d in /opt/homebrew/bin /usr/local/bin; do [ -x "$d/npm" ] && export PATH="$d:$PATH"; done
fi
if ! command -v npm >/dev/null 2>&1; then
  if command -v brew >/dev/null 2>&1; then
    echo "Missing: Node.js.  Install:  brew install node"
  else
    echo "Missing: Node.js.  Install the LTS version from https://nodejs.org (macOS Installer .pkg),"
    echo "         then open a new terminal window."
  fi
  missing=1
fi
[ "$missing" = 0 ] || { echo; echo "Install the missing tools above, then run this script again."; exit 1; }

if [ ! -f ui/dist/index.html ] || [ -n "${REBUILD_UI:-}" ]; then
  (cd ui && npm ci && npm run build)
fi
rustup target add aarch64-apple-darwin x86_64-apple-darwin
cargo build --release --locked -p showmaster --target aarch64-apple-darwin
cargo build --release --locked -p showmaster --target x86_64-apple-darwin

V=$(cargo metadata --no-deps --format-version 1 | python3 -c 'import json,sys; print([p["version"] for p in json.load(sys.stdin)["packages"] if p["name"]=="showmaster"][0])')
WORK=$(mktemp -d)
APP="$WORK/dmg/ShowMaster.app"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources" dist
lipo -create -output "$APP/Contents/MacOS/showmaster" target/aarch64-apple-darwin/release/showmaster target/x86_64-apple-darwin/release/showmaster
sed "s/APP_VERSION/$V/g" packaging/macos/Info.plist > "$APP/Contents/Info.plist"
cp README.md "$APP/Contents/Resources/"
ICONSET="$WORK/ShowMaster.iconset"
mkdir -p "$ICONSET"
for s in 16 32 128 256 512; do
  sips -z $s $s assets/icon.png --out "$ICONSET/icon_${s}x${s}.png" >/dev/null
  sips -z $((s*2)) $((s*2)) assets/icon.png --out "$ICONSET/icon_${s}x${s}@2x.png" >/dev/null
done
iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/ShowMaster.icns"
# Ad-hoc signature (no Apple Developer certificate): macOS asks once for "Open Anyway".
codesign --force --deep -s - "$APP"
codesign --verify --deep "$APP"
lipo -info "$APP/Contents/MacOS/showmaster"
ln -s /Applications "$WORK/dmg/Applications"
rm -f "dist/ShowMaster-$V.dmg"
hdiutil create -volname "Show Master" -srcfolder "$WORK/dmg" -ov -format UDZO "dist/ShowMaster-$V.dmg"
# Program-only downloads for the in-app updater.
gzip -9 -c target/aarch64-apple-darwin/release/showmaster > dist/showmaster-macos-arm64.gz
gzip -9 -c target/x86_64-apple-darwin/release/showmaster > dist/showmaster-macos-x86_64.gz
rm -rf "$WORK"
ls -la dist
echo "Built dist/ShowMaster-$V.dmg"
