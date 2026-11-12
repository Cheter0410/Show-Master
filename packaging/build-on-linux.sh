#!/bin/bash
# Builds everything for Linux and Windows on one Linux PC (Arch, Debian/Ubuntu, ...):
#   dist/showmaster-<v>-linux-x86_64.tar.gz    portable Linux program (any distribution)
#   dist/showmaster-<v>-1-x86_64.pkg.tar.zst   Arch Linux package          (if makepkg exists)
#   dist/showmaster_<v>-1_amd64.deb            Debian/Ubuntu package        (if cargo-deb exists)
#   dist/ShowMaster-<v>-x86_64.AppImage        AppImage (if appimagetool exists, or APPIMAGE=1 downloads it)
#   dist/ShowMaster-Setup-<v>.exe              Windows installer (Inno Setup under Wine)
#   dist/showmaster-windows-x86_64.gz          program-only download for the Windows updater
# The list of what belongs to this build is written to dist/files-linux-pc.txt
# (release.sh uploads exactly these).
#
# Nothing is done twice: the tests run once per commit, and every package is
# only made again when the program it contains changed.
# Skip parts with SKIP_WINDOWS=1 / SKIP_TESTS=1 / SKIP_APPIMAGE=1.
# Optional Windows code signing (only when set; needs osslsigncode):
#   WIN_SIGN_PFX=/path/cert.pfx  WIN_SIGN_PASSWORD=...  [WIN_SIGN_TIMESTAMP_URL=http://...]
set -euo pipefail
cd "$(dirname "$0")/.."
# shellcheck source=../scripts/ui.sh
. scripts/ui.sh
V=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)
mkdir -p dist target
FILES=()
ui_header "Show Master $V" "Linux + Windows bauen"

# The built web UI is committed; restore it if it was deleted locally.
[ -f ui/dist/index.html ] || git checkout HEAD -- ui/dist

# --- Tests: once per commit ------------------------------------------------
tests() {
  # No real audio device: the tests must not disturb this computer's sound.
  SHOWMASTER_NO_AUDIO_DEVICES=1 cargo test --workspace --locked
}
if [ -n "${SKIP_TESTS:-}" ]; then
  ui_skip "Tests" "übersprungen (SKIP_TESTS)"
else
  key=""
  if [ -z "$(git status --porcelain --untracked-files=no 2>/dev/null)" ]; then
    key=$(git rev-parse HEAD 2>/dev/null || true)
  fi
  if [ -n "$key" ] && [ -f "target/.tested/$key" ]; then
    ui_skip "Tests" "für diesen Stand schon bestanden"
  else
    ui_run "Tests" tests
    [ -n "$key" ] && mkdir -p target/.tested && touch "target/.tested/$key"
  fi
fi

# --- Linux -----------------------------------------------------------------
ui_run "Linux-Programm" cargo build --release --locked -p showmaster
BIN=target/release/showmaster
KEY="$V-$(ui_fingerprint "$BIN")"

NAME="showmaster-$V-linux-x86_64"
TAR="dist/$NAME.tar.gz"
make_tar() {
  rm -rf "dist/$NAME" && mkdir -p "dist/$NAME"
  cp "$BIN" README.md packaging/linux/showmaster.desktop assets/icon.png "dist/$NAME/"
  tar -C dist -czf "$TAR" "$NAME" && rm -rf "dist/$NAME"
}
if ui_fresh "$TAR" "$KEY"; then ui_skip "Linux-Archiv" "unverändert"; else
  ui_run "Linux-Archiv" make_tar && ui_made "$TAR" "$KEY"
fi
FILES+=("$TAR")

if command -v makepkg >/dev/null 2>&1; then
  PKG=$(ls dist/showmaster-"$V"-*-x86_64.pkg.tar.zst 2>/dev/null | head -n 1 || true)
  make_pkg() {
    rm -f dist/showmaster-"$V"-*-x86_64.pkg.tar.zst
    (cd packaging/arch && rm -f ./*.pkg.tar.* && SM_VERSION="$V" makepkg -f --noconfirm --nodeps && mv ./*.pkg.tar.zst ../../dist/)
    rm -rf packaging/arch/pkg packaging/arch/src
  }
  if [ -n "$PKG" ] && ui_fresh "$PKG" "$KEY"; then ui_skip "Arch-Paket" "unverändert"; else
    ui_run "Arch-Paket" make_pkg
    PKG=$(ls dist/showmaster-"$V"-*-x86_64.pkg.tar.zst | head -n 1)
    ui_made "$PKG" "$KEY"
  fi
  FILES+=("$PKG")
fi

if command -v cargo-deb >/dev/null 2>&1; then
  DEB=$(ls dist/showmaster_"$V"*.deb 2>/dev/null | head -n 1 || true)
  make_deb() { cargo deb -p showmaster --locked --no-build && cp target/debian/showmaster_"$V"*.deb dist/; }
  if [ -n "$DEB" ] && ui_fresh "$DEB" "$KEY"; then ui_skip "Debian-Paket" "unverändert"; else
    ui_run "Debian-Paket" make_deb
    DEB=$(ls dist/showmaster_"$V"*.deb | head -n 1)
    ui_made "$DEB" "$KEY"
  fi
  FILES+=("$DEB")
fi

# AppImage: one file that runs on most distributions. Built when appimagetool is
# installed (AUR: appimagetool-bin) or downloaded into a cache folder with APPIMAGE=1.
APPIMAGETOOL=$(command -v appimagetool || true)
if [ -z "${SKIP_APPIMAGE:-}" ] && [ -z "$APPIMAGETOOL" ] && [ -n "${APPIMAGE:-}" ]; then
  CACHE="${XDG_CACHE_HOME:-$HOME/.cache}/showmaster-build"
  APPIMAGETOOL="$CACHE/appimagetool-x86_64.AppImage"
  if [ ! -x "$APPIMAGETOOL" ]; then
    get_tool() {
      mkdir -p "$CACHE"
      curl -fL --retry 3 -o "$APPIMAGETOOL.part" \
        "${APPIMAGETOOL_URL:-https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-x86_64.AppImage}"
      chmod +x "$APPIMAGETOOL.part" && mv "$APPIMAGETOOL.part" "$APPIMAGETOOL"
    }
    ui_run "appimagetool laden (einmalig)" get_tool
  fi
fi
if [ -z "${SKIP_APPIMAGE:-}" ] && [ -n "$APPIMAGETOOL" ]; then
  OUT="dist/ShowMaster-$V-x86_64.AppImage"
  make_appimage() {
    APPDIR=target/appimage/ShowMaster.AppDir
    rm -rf target/appimage && mkdir -p "$APPDIR/usr/bin" "$APPDIR/usr/share/applications" \
      "$APPDIR/usr/share/icons/hicolor/256x256/apps"
    cp "$BIN" "$APPDIR/usr/bin/"
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
    rm -f "$OUT"
    # APPIMAGE_EXTRACT_AND_RUN: appimagetool is itself an AppImage; this works without FUSE.
    ARCH=x86_64 APPIMAGE_EXTRACT_AND_RUN=1 "$APPIMAGETOOL" --no-appstream "$APPDIR" "$OUT"
    rm -rf target/appimage
  }
  if ui_fresh "$OUT" "$KEY"; then ui_skip "AppImage" "unverändert"; else
    ui_run "AppImage" make_appimage && ui_made "$OUT" "$KEY"
  fi
  FILES+=("$OUT")
fi

# --- Windows ---------------------------------------------------------------
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
  command -v osslsigncode >/dev/null 2>&1 || ui_fail "WIN_SIGN_PFX is set but osslsigncode is missing (Arch: sudo pacman -S osslsigncode)."
  [ -f "$WIN_SIGN_PFX" ] || ui_fail "WIN_SIGN_PFX: file not found: $WIN_SIGN_PFX"
fi

if [ -z "${SKIP_WINDOWS:-}" ]; then
  ui_run "Windows-Programm" cargo build --release --locked -p showmaster --target x86_64-pc-windows-gnu
  EXE=target/x86_64-pc-windows-gnu/release/showmaster.exe
  WKEY="$V-$(ui_fingerprint "$EXE")${WIN_SIGN_PFX:+-signed}"
  if [ -n "${WIN_SIGN_PFX:-}" ]; then
    mkdir -p target/x86_64-pc-windows-gnu/release/signed
    ui_run "Programm signieren" sign_exe "$EXE" target/x86_64-pc-windows-gnu/release/signed/showmaster.exe
    EXE=target/x86_64-pc-windows-gnu/release/signed/showmaster.exe
  fi
  GZ=dist/showmaster-windows-x86_64.gz
  if ui_fresh "$GZ" "$WKEY"; then ui_skip "Windows-Update-Datei" "unverändert"; else
    ui_run "Windows-Update-Datei" sh -c "gzip -9 -c '$EXE' > '$GZ'" && ui_made "$GZ" "$WKEY"
  fi
  FILES+=("$GZ")

  ISCC="${ISCC:-$HOME/.wine/drive_c/InnoSetup/ISCC.exe}"
  WINE=$(command -v wine || command -v wine64 || ls /usr/lib/wine/wine64 2>/dev/null || true)
  export WINEDEBUG=-all WINEDLLOVERRIDES="mscoree,mshtml="
  SETUP="dist/ShowMaster-Setup-$V.exe"
  make_setup() {
    rm -rf stage && mkdir -p stage
    cp "$EXE" README.md stage/
    # Wine maps the Linux root to drive Z:.
    "$WINE" "$ISCC" "/DAppVersion=$V" "Z:$(pwd)/packaging/windows/showmaster.iss"
    rm -rf stage
    if [ -n "${WIN_SIGN_PFX:-}" ]; then
      sign_exe "$SETUP" "$SETUP.signed" && mv "$SETUP.signed" "$SETUP"
    fi
  }
  if [ -n "$WINE" ] && [ -f "$ISCC" ]; then
    if ui_fresh "$SETUP" "$WKEY"; then ui_skip "Windows-Installer" "unverändert"; else
      ui_run "Windows-Installer" make_setup && ui_made "$SETUP" "$WKEY"
    fi
    FILES+=("$SETUP")
  else
    ui_warn "Windows-Installer übersprungen: Wine oder Inno Setup fehlt (release.sh richtet beides ein)"
  fi
fi

{ echo "$V"; printf '%s\n' "${FILES[@]}"; } > dist/files-linux-pc.txt
ui_done
