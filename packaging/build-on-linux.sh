#!/bin/bash
# Builds everything for Linux and Windows on one Linux PC (Arch, Debian/Ubuntu, ...):
#   dist/showmaster-<v>-linux-x86_64.tar.gz    portable Linux program (any distribution)
#   dist/showmaster-<v>-1-x86_64.pkg.tar.zst   Arch Linux package          (if makepkg exists)
#   dist/showmaster_<v>-1_amd64.deb            Debian/Ubuntu package        (if cargo-deb exists)
#   dist/ShowMaster-Setup-<v>.exe              Windows installer (Inno Setup under Wine)
#   dist/showmaster-windows-x86_64.gz          program-only download for the Windows updater
# One-time setup: docs/release-anleitung.md. Skip parts with SKIP_WINDOWS=1 / SKIP_TESTS=1.
set -euo pipefail
cd "$(dirname "$0")/.."
V=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)
mkdir -p dist

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

if [ -z "${SKIP_WINDOWS:-}" ]; then
  echo "== Windows program"
  rustup target add x86_64-pc-windows-gnu >/dev/null
  cargo build --release --locked -p showmaster --target x86_64-pc-windows-gnu
  EXE=target/x86_64-pc-windows-gnu/release/showmaster.exe
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
  else
    echo "Wine or Inno Setup ($ISCC) not found - skipping the Windows installer (see docs/release-anleitung.md)."
  fi
fi

ls -la dist
