#!/usr/bin/env bash
# Builds the portable distribution: one self-contained executable (web UI
# embedded) plus docs and the demo show generator. No installation needed.
set -euo pipefail
cd "$(dirname "$0")/.."
cargo build --release --locked -p showmaster
VERSION=$(cargo metadata --no-deps --format-version 1 | python3 -c 'import json,sys; print([p["version"] for p in json.load(sys.stdin)["packages"] if p["name"]=="showmaster"][0])')
case "$(uname -s)" in
  Linux*) OS=linux ;; Darwin*) OS=macos ;; *) OS=windows ;;
esac
ARCH=$(uname -m)
NAME="showmaster-$VERSION-$OS-$ARCH"
rm -rf "dist/$NAME" && mkdir -p "dist/$NAME"
if [ "$OS" = windows ]; then cp target/release/showmaster.exe "dist/$NAME/"; else cp target/release/showmaster "dist/$NAME/"; fi
cp README.md "dist/$NAME/"
cp -r docs "dist/$NAME/docs"
cd dist
if [ "$OS" = windows ]; then
  powershell -NoProfile -Command "Compress-Archive -Force -Path '$NAME' -DestinationPath '$NAME.zip'"
else
  tar czf "$NAME.tar.gz" "$NAME"
fi
rm -rf "$NAME"
ls -lh
