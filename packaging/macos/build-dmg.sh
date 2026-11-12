#!/bin/bash
# Builds ShowMaster-x.y.z.dmg on a Mac, for this Mac's architecture only by
# default (one compile). UNIVERSAL=1 builds Apple Silicon + Intel (two
# compiles, for publishing); ARCHS="x86_64" or ARCHS="arm64" picks one.
# Needs: Xcode command line tools and Rust (rustup). Node.js only with REBUILD_UI=1.
#   ./packaging/macos/build-dmg.sh
# The result is in dist/. Used by the `build` workflow and for building by hand.
# Optional signing (only when set; otherwise the app is ad-hoc signed as before):
#   SIGN_IDENTITY="Developer ID Application: Name (TEAMID)"  sign with hardened runtime
#   NOTARY_PROFILE=showmaster   also notarize + staple (profile from
#                               `xcrun notarytool store-credentials showmaster ...`)
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
# Node.js is only needed to rebuild the web UI; the built UI is part of the repository.
if [ -n "${REBUILD_UI:-}" ] && ! command -v npm >/dev/null 2>&1; then
  for d in /opt/homebrew/bin /usr/local/bin; do [ -x "$d/npm" ] && export PATH="$d:$PATH"; done
fi
if [ -n "${REBUILD_UI:-}" ] && ! command -v npm >/dev/null 2>&1; then
  if command -v brew >/dev/null 2>&1; then
    echo "Missing: Node.js.  Install:  brew install node"
  else
    echo "Missing: Node.js.  Install the LTS version from https://nodejs.org (macOS Installer .pkg),"
    echo "         then open a new terminal window."
  fi
  missing=1
fi
if [ -n "${NOTARY_PROFILE:-}" ] && [ -z "${SIGN_IDENTITY:-}" ]; then
  echo "NOTARY_PROFILE is set but SIGN_IDENTITY is not: notarization needs a Developer ID signature."; missing=1
fi
if [ -n "${SIGN_IDENTITY:-}" ] && ! security find-identity -v -p codesigning 2>/dev/null | grep -F "$SIGN_IDENTITY" >/dev/null; then
  echo "Signing identity not found in the keychain: $SIGN_IDENTITY"
  echo "         List them with:  security find-identity -v -p codesigning"; missing=1
fi
[ "$missing" = 0 ] || { echo; echo "Install the missing tools above, then run this script again."; exit 1; }

# The built UI is part of the repository; restore it if it was deleted locally.
if [ ! -f ui/dist/index.html ] && [ -z "${REBUILD_UI:-}" ]; then
  git checkout HEAD -- ui/dist 2>/dev/null || true
fi
if [ ! -f ui/dist/index.html ] || [ -n "${REBUILD_UI:-}" ]; then
  (cd ui && npm ci && npm run build)
fi
mkdir -p dist && touch dist/.build-start   # release.sh uploads only newer files
# This Mac's own architecture (also when the terminal runs under Rosetta).
if [ "$(sysctl -n hw.optional.arm64 2>/dev/null || echo 0)" = "1" ]; then HOST=arm64; else HOST=x86_64; fi
ARCHS="${ARCHS:-$HOST}"
[ -n "${UNIVERSAL:-}" ] && ARCHS="arm64 x86_64"
BINS=()
for a in $ARCHS; do
  case "$a" in
    arm64|aarch64) a=arm64; t=aarch64-apple-darwin ;;
    x86_64|intel) a=x86_64; t=x86_64-apple-darwin ;;
    *) echo "Unknown architecture in ARCHS: $a (use arm64 and/or x86_64)"; exit 1 ;;
  esac
  if [ "$a" = "$HOST" ] && [ "$(uname -m)" = "$HOST" ]; then
    # Native build without --target: build tools and the app share one
    # compile (with --target cargo builds the build-time crates separately,
    # which looks like everything is compiled twice).
    echo "== Building for this Mac ($a)"
    cargo build --release --locked -p showmaster
    BINS+=("target/release/showmaster")
  else
    echo "== Building for $t"
    rustup target add "$t"
    cargo build --release --locked -p showmaster --target "$t"
    BINS+=("target/$t/release/showmaster")
  fi
done

V=$(cargo metadata --no-deps --format-version 1 | python3 -c 'import json,sys; print([p["version"] for p in json.load(sys.stdin)["packages"] if p["name"]=="showmaster"][0])')
WORK=$(mktemp -d)
APP="$WORK/dmg/ShowMaster.app"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources" dist
lipo -create -output "$APP/Contents/MacOS/showmaster" "${BINS[@]}"
sed "s/APP_VERSION/$V/g" packaging/macos/Info.plist > "$APP/Contents/Info.plist"
cp README.md "$APP/Contents/Resources/"
ICONSET="$WORK/ShowMaster.iconset"
mkdir -p "$ICONSET"
for s in 16 32 128 256 512; do
  sips -z $s $s assets/icon.png --out "$ICONSET/icon_${s}x${s}.png" >/dev/null
  sips -z $((s*2)) $((s*2)) assets/icon.png --out "$ICONSET/icon_${s}x${s}@2x.png" >/dev/null
done
iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/ShowMaster.icns"
# notarize <file>: submit to Apple, wait, fail with the log unless accepted.
notarize() {
  local out id status
  out=$(xcrun notarytool submit "$1" --keychain-profile "$NOTARY_PROFILE" --wait --output-format json)
  echo "$out"
  id=$(printf '%s' "$out" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("id",""))')
  status=$(printf '%s' "$out" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("status",""))')
  if [ "$status" != "Accepted" ]; then
    echo "Notarization of $1 failed ($status)."
    [ -n "$id" ] && xcrun notarytool log "$id" --keychain-profile "$NOTARY_PROFILE" || true
    exit 1
  fi
}
if [ -n "${SIGN_IDENTITY:-}" ]; then
  # Developer ID with hardened runtime and secure timestamp (required for notarization).
  # Entitlements: audio input only (LTC timecode from an audio input); see the file.
  echo "== Signing with $SIGN_IDENTITY"
  codesign --force --options runtime --timestamp \
    --entitlements packaging/macos/entitlements.plist \
    --sign "$SIGN_IDENTITY" "$APP"
  codesign --verify --strict --deep --verbose=2 "$APP"
  if [ -n "${NOTARY_PROFILE:-}" ]; then
    # Notarize and staple the app itself too: show computers are often offline, and a
    # stapled ticket lets Gatekeeper check the app without asking Apple's servers.
    echo "== Notarizing the app"
    ditto -c -k --keepParent "$APP" "$WORK/ShowMaster.zip"
    notarize "$WORK/ShowMaster.zip"
    xcrun stapler staple "$APP"
    rm -f "$WORK/ShowMaster.zip"
  fi
else
  # Ad-hoc signature (no Apple Developer certificate): macOS asks once for "Open Anyway".
  codesign --force --deep -s - "$APP"
  codesign --verify --deep "$APP"
fi
lipo -info "$APP/Contents/MacOS/showmaster"
ln -s /Applications "$WORK/dmg/Applications"
rm -f "dist/ShowMaster-$V.dmg"
hdiutil create -volname "Show Master" -srcfolder "$WORK/dmg" -ov -format UDZO "dist/ShowMaster-$V.dmg"
if [ -n "${SIGN_IDENTITY:-}" ]; then
  codesign --force --timestamp --sign "$SIGN_IDENTITY" "dist/ShowMaster-$V.dmg"
  if [ -n "${NOTARY_PROFILE:-}" ]; then
    echo "== Notarizing the DMG"
    notarize "dist/ShowMaster-$V.dmg"
    xcrun stapler staple "dist/ShowMaster-$V.dmg"
    xcrun stapler validate "dist/ShowMaster-$V.dmg"
    spctl --assess --type open --context context:primary-signature --verbose "dist/ShowMaster-$V.dmg"
  fi
fi
# Program-only downloads for the in-app updater.
for b in "${BINS[@]}"; do
  case "$b" in
    *aarch64*) gzip -9 -c "$b" > dist/showmaster-macos-arm64.gz ;;
    *) gzip -9 -c "$b" > dist/showmaster-macos-x86_64.gz ;;
  esac
done
rm -rf "$WORK"
ls -la dist
echo "Built dist/ShowMaster-$V.dmg"
