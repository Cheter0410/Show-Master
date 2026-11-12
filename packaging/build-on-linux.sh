#!/bin/bash
# Builds everything for Linux and Windows on one Linux PC (Arch, Debian/Ubuntu, ...):
#   dist/showmaster-<v>-linux-x86_64.tar.gz    portable Linux program (any distribution)
#   dist/showmaster-<v>-1-x86_64.pkg.tar.zst   Arch Linux package          (if makepkg exists)
#   dist/showmaster_<v>-1_amd64.deb            Debian/Ubuntu package        (if cargo-deb exists)
#   dist/ShowMaster-<v>-x86_64.AppImage        AppImage (if appimagetool exists, or APPIMAGE=1 downloads it)
#   dist/ShowMaster-Setup-<v>.exe              Windows installer (Inno Setup under Wine)
#   dist/showmaster-windows-x86_64.gz          program-only download for the Windows updater
# One-time setup: docs/release-anleitung.md. Skip parts with SKIP_WINDOWS=1 / SKIP_TESTS=1 /
# SKIP_APPIMAGE=1.
# Optional Windows code signing (only when set; needs osslsigncode):
#   WIN_SIGN_PFX=/path/cert.pfx  WIN_SIGN_PASSWORD=...  [WIN_SIGN_TIMESTAMP_URL=http://...]
set -euo pipefail
cd "$(dirname "$0")/.."
V=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)
mkdir -p dist
touch dist/.build-start   # release.sh uploads only files newer than this

# The built web UI is committed; restore it if it was deleted locally.
[ -f ui/dist/index.html ] || git checkout HEAD -- ui/dist

if [ -z "${SKIP_TESTS:-}" ]; then
  echo "== Tests"
  cargo test --workspace --locked
fi

echo "== Linux program"
cargo build --release --locked -p showmaster
NAME="showmaster-$V-linux-x86_64"
rm -rf "dist/$NAME" && mkdir -p "dist/$NAME"
cp target/release/showmaster README.md "dist/$NAME/"
cp packaging/linux/showmaster.desktop assets/icon.png "dist/$NAME/"
tar -C dist -czf "dist/$NAME.tar.gz" "$NAME" && rm -rf "dist/$NAME"

if command -v makepkg >/dev/null 2>&1; then
  echo "== Arch Linux package"
  (cd packaging/arch && rm -f ./*.pkg.tar.* && SM_VERSION="$V" makepkg -f --noconfirm --nodeps >/dev/null && mv ./*.pkg.tar.zst ../../dist/)
  rm -rf packaging/arch/pkg packaging/arch/src
fi

if command -v cargo-deb >/dev/null 2>&1; then
  echo "== Debian package"
  cargo deb -p showmaster --locked --no-build
  cp target/debian/showmaster_"$V"*.deb dist/
fi

# AppImage: one file that runs on most distributions. Built when appimagetool is
# installed (AUR: appimagetool-bin) or downloaded into a cache folder with APPIMAGE=1.
APPIMAGETOOL=$(command -v appimagetool || true)
if [ -z "${SKIP_APPIMAGE:-}" ] && [ -z "$APPIMAGETOOL" ] && [ -n "${APPIMAGE:-}" ]; then
  CACHE="${XDG_CACHE_HOME:-$HOME/.cache}/showmaster-build"
  APPIMAGETOOL="$CACHE/appimagetool-x86_64.AppImage"
  if [ ! -x "$APPIMAGETOOL" ]; then
    echo "== Downloading appimagetool into $CACHE"
    mkdir -p "$CACHE"
    curl -fL --retry 3 -o "$APPIMAGETOOL.part" \
      "${APPIMAGETOOL_URL:-https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-x86_64.AppImage}"
    chmod +x "$APPIMAGETOOL.part" && mv "$APPIMAGETOOL.part" "$APPIMAGETOOL"
  fi
fi
if [ -z "${SKIP_APPIMAGE:-}" ] && [ -n "$APPIMAGETOOL" ]; then
  echo "== AppImage"
  APPDIR=target/appimage/ShowMaster.AppDir
  rm -rf target/appimage && mkdir -p "$APPDIR/usr/bin" "$APPDIR/usr/share/applications" \
    "$APPDIR/usr/share/icons/hicolor/256x256/apps"
  cp target/release/showmaster "$APPDIR/usr/bin/"
  cp packaging/linux/showmaster.desktop "$APPDIR/showmaster.desktop"
  cp packaging/linux/showmaster.desktop "$APPDIR/usr/share/applications/"
  cp packaging/linux/AppRun "$APPDIR/AppRun" && chmod +x "$APPDIR/AppRun"
  if command -v convert >/dev/null 2>&1; then
    convert assets/icon.png -resize 256x256 "$APPDIR/showmaster.png"
  else
    cp assets/icon.png "$APPDIR/showmaster.png"
  fi
  cp "$APPDIR/showmaster.png" "$APPDIR/usr/share/icons/hicolor/256x256/apps/showmaster.png"
  ln -sf showmaster.png "$APPDIR/.DirIcon"
  chmod -R u+rwX,go+rX "$APPDIR"
  OUT="dist/ShowMaster-$V-x86_64.AppImage"
  rm -f "$OUT"
  # APPIMAGE_EXTRACT_AND_RUN: appimagetool is itself an AppImage; this works without FUSE.
  ARCH=x86_64 APPIMAGE_EXTRACT_AND_RUN=1 "$APPIMAGETOOL" --no-appstream "$APPDIR" "$OUT"
  rm -rf target/appimage
fi

# Optional Authenticode signing (osslsigncode): sign_exe <in> <out>
sign_exe() {
  local pass_file
  pass_file=$(mktemp)
  chmod 600 "$pass_file"
  printf '%s' "${WIN_SIGN_PASSWORD:-}" > "$pass_file"
  rm -f "$2"
  osslsigncode sign -pkcs12 "$WIN_SIGN_PFX" -readpass "$pass_file" \
    -n "Show Master" -h sha256 \
    -ts "${WIN_SIGN_TIMESTAMP_URL:-http://timestamp.digicert.com}" \
    -in "$1" -out "$2" || { rm -f "$pass_file"; return 1; }
  rm -f "$pass_file"
}
if [ -z "${SKIP_WINDOWS:-}" ] && [ -n "${WIN_SIGN_PFX:-}" ]; then
  command -v osslsigncode >/dev/null 2>&1 || {
    echo "WIN_SIGN_PFX is set but osslsigncode is missing (Arch: sudo pacman -S osslsigncode)."; exit 1; }
  [ -f "$WIN_SIGN_PFX" ] || { echo "WIN_SIGN_PFX: file not found: $WIN_SIGN_PFX"; exit 1; }
fi

if [ -z "${SKIP_WINDOWS:-}" ]; then
  echo "== Windows program"
  rustup target add x86_64-pc-windows-gnu >/dev/null
  cargo build --release --locked -p showmaster --target x86_64-pc-windows-gnu
  EXE=target/x86_64-pc-windows-gnu/release/showmaster.exe
  if [ -n "${WIN_SIGN_PFX:-}" ]; then
    echo "== Signing showmaster.exe"
    mkdir -p target/x86_64-pc-windows-gnu/release/signed
    sign_exe "$EXE" target/x86_64-pc-windows-gnu/release/signed/showmaster.exe
    EXE=target/x86_64-pc-windows-gnu/release/signed/showmaster.exe
  fi
  gzip -9 -c "$EXE" > dist/showmaster-windows-x86_64.gz

  echo "== Windows installer"
  ISCC="${ISCC:-$HOME/.wine/drive_c/InnoSetup/ISCC.exe}"
  WINE=$(command -v wine || command -v wine64 || ls /usr/lib/wine/wine64 2>/dev/null || true)
  export WINEDEBUG=-all WINEDLLOVERRIDES="mscoree,mshtml="
  if [ -n "$WINE" ] && [ -f "$ISCC" ]; then
    rm -rf stage && mkdir -p stage
    cp "$EXE" README.md stage/
    # Wine maps the Linux root to drive Z:.
    "$WINE" "$ISCC" "/DAppVersion=$V" "Z:$(pwd)/packaging/windows/showmaster.iss"
    rm -rf stage
    if [ -n "${WIN_SIGN_PFX:-}" ]; then
      echo "== Signing the installer"
      SETUP="dist/ShowMaster-Setup-$V.exe"
      sign_exe "$SETUP" "$SETUP.signed"
      mv "$SETUP.signed" "$SETUP"
    fi
  else
    echo "Wine or Inno Setup ($ISCC) not found - skipping the Windows installer (see docs/release-anleitung.md)."
  fi
fi

ls -la dist
