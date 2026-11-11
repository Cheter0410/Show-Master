#!/bin/bash
# Builds on a Linux PC (Debian/Ubuntu, x86_64):
#   dist/showmaster_<v>-1_amd64.deb           Linux package
#   dist/ShowMaster-Setup-<v>.exe             Windows installer (Inno Setup, run through Wine)
#   dist/showmaster-windows-x86_64.gz         program-only download for the Windows updater
# One-time setup: see docs/release-anleitung.md (section "Linux-PC einrichten").
set -euo pipefail
cd "$(dirname "$0")/.."
V=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)
ISCC="${ISCC:-$HOME/.wine/drive_c/InnoSetup/ISCC.exe}"
mkdir -p dist

echo "== Web UI"
(cd ui && npm ci && npm run build)

echo "== Tests"
cargo test --workspace --locked

echo "== Linux .deb"
cargo deb -p showmaster --locked
cp target/debian/showmaster_"$V"*.deb dist/

echo "== Windows program"
cargo build --release --locked -p showmaster --target x86_64-pc-windows-gnu
EXE=target/x86_64-pc-windows-gnu/release/showmaster.exe
gzip -9 -c "$EXE" > dist/showmaster-windows-x86_64.gz

echo "== Windows installer"
if [ -f "$ISCC" ]; then
  rm -rf stage && mkdir -p stage
  cp "$EXE" stage/
  cp README.md stage/
  # Wine maps the Linux root to drive Z:.
  WINEDEBUG=-all wine "$ISCC" "/DAppVersion=$V" "Z:$(pwd)/packaging/windows/showmaster.iss"
  rm -rf stage
else
  echo "Inno Setup not found at $ISCC - skipping the Windows installer (see docs/release-anleitung.md)."
fi

ls -la dist
