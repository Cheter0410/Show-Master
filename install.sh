#!/bin/bash
# Show Master on Linux: build from this checkout and install, in one go.
#
#   git clone https://github.com/Cheter0410/Show-Master.git
#   cd Show-Master
#   ./install.sh
#
# Run it again later to update (it pulls the newest version first).
# Arch Linux: builds and installs a real package (pacman -U), so
# `sudo pacman -R showmaster` removes it again. Other distributions: installs
# to /usr/local (remove with ./install.sh --uninstall).
set -euo pipefail
cd "$(dirname "$0")"

say() { printf '\n\033[1m== %s\033[0m\n' "$*"; }

if [ "${1:-}" = "--uninstall" ]; then
  if command -v pacman >/dev/null 2>&1 && pacman -Q showmaster >/dev/null 2>&1; then
    sudo pacman -R --noconfirm showmaster
  else
    sudo rm -f /usr/local/bin/showmaster /usr/local/share/applications/showmaster.desktop \
      /usr/local/share/icons/hicolor/512x512/apps/showmaster.png
  fi
  echo "Show Master removed (your shows and settings stay where they are)."
  exit 0
fi

if [ -d .git ] && git diff --quiet && git diff --cached --quiet; then
  say "Fetching the newest version"
  git pull --ff-only || echo "(could not update - building the version that is here)"
fi

say "Installing what the build needs"
if command -v pacman >/dev/null 2>&1; then
  pkgs=(git base-devel alsa-lib gtk3 libxkbcommon-x11 libglvnd libcap)
  command -v cargo >/dev/null 2>&1 || pkgs+=(rust)
  sudo pacman -S --needed --noconfirm "${pkgs[@]}"
elif command -v apt-get >/dev/null 2>&1; then
  sudo apt-get update
  sudo apt-get install -y git build-essential pkg-config libasound2-dev libgtk-3-dev \
    libxkbcommon-x11-dev libcap2-bin curl
elif command -v dnf >/dev/null 2>&1; then
  sudo dnf install -y git gcc pkgconf-pkg-config alsa-lib-devel gtk3-devel libxkbcommon-x11-devel \
    libcap curl
else
  echo "Unknown distribution: install a C compiler, pkg-config and the ALSA / GTK3 development"
  echo "packages yourself, then run this script again."
fi
if ! command -v cargo >/dev/null 2>&1; then
  say "Installing Rust (rustup, for this user)"
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal
  # shellcheck disable=SC1091
  . "$HOME/.cargo/env"
fi

# The built web UI is committed; restore it if it was deleted locally.
[ -f ui/dist/index.html ] || git checkout HEAD -- ui/dist

V=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)

if command -v makepkg >/dev/null 2>&1 && command -v pacman >/dev/null 2>&1; then
  say "Building Show Master $V (takes a few minutes the first time)"
  cargo build --release --locked -p showmaster
  say "Building the Arch package"
  (cd packaging/arch && rm -f ./*.pkg.tar.* && SM_VERSION="$V" makepkg -f --noconfirm --nodeps >/dev/null)
  rm -rf packaging/arch/pkg packaging/arch/src
  say "Installing"
  sudo pacman -U --noconfirm packaging/arch/showmaster-"$V"-*.pkg.tar.zst
  rm -f packaging/arch/*.pkg.tar.*
else
  say "Building Show Master $V (takes a few minutes the first time)"
  cargo build --release --locked -p showmaster
  say "Installing to /usr/local"
  sudo install -Dm755 target/release/showmaster /usr/local/bin/showmaster
  sudo install -Dm644 packaging/linux/showmaster.desktop /usr/local/share/applications/showmaster.desktop
  sudo install -Dm644 assets/icon.png /usr/local/share/icons/hicolor/512x512/apps/showmaster.png
  # The standard web port 80, so phones only type showmaster.local.
  sudo setcap 'cap_net_bind_service=+ep' /usr/local/bin/showmaster 2>/dev/null || true
fi

say "Done"
echo "Start Show Master from the application menu or with: showmaster"
echo "On a phone or tablet in the same network: showmaster.local"
