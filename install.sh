#!/bin/bash
# Show Master on Linux: update this checkout, build and install - in one go.
#
#   git clone https://github.com/Cheter0410/Show-Master.git
#   cd Show-Master
#   ./install.sh
#
# Run it again later to update. Only what is needed is done: packages that are
# there are not installed again, an unchanged program is not reinstalled.
# Arch Linux: installs a real package (sudo pacman -R showmaster removes it).
# Other distributions: /usr/local (remove with ./install.sh --uninstall).
set -euo pipefail
cd "$(dirname "$0")"
# shellcheck source=scripts/ui.sh
. scripts/ui.sh

if [ "${1:-}" = "--uninstall" ]; then
  ui_header "Show Master" "Entfernen"
  ui_sudo
  if command -v pacman >/dev/null 2>&1 && pacman -Q showmaster >/dev/null 2>&1; then
    ui_run "Paket entfernen" sudo pacman -R --noconfirm showmaster
  else
    ui_run "Dateien entfernen" sudo rm -f /usr/local/bin/showmaster /usr/local/share/applications/showmaster.desktop \
      /usr/local/share/icons/hicolor/512x512/apps/showmaster.png
  fi
  ui_ok "Entfernt (Shows und Einstellungen bleiben)"
  exit 0
fi

# --- Neuester Stand (danach läuft die neue Fassung dieses Skripts) ---------
if [ -z "${SM_PULLED:-}" ] && [ -d .git ]; then
  ui_header "Show Master" "Installieren / Aktualisieren"
  if git diff --quiet && git diff --cached --quiet; then
    git fetch -q 2>/dev/null || true
    if [ -n "$(git rev-list HEAD..@{u} 2>/dev/null)" ]; then
      ui_run "Neuesten Stand holen" git pull -q --ff-only
      SM_PULLED=1 exec bash "$(pwd)/$(basename "$0")" "$@"
    fi
    ui_ok "Quellcode ist aktuell"
  else
    ui_warn "Lokale Änderungen – baue den Stand, der hier liegt"
  fi
else
  ui_ok "Quellcode ist aktuell"
fi

# --- Werkzeuge: nur was fehlt -------------------------------------------------
missing=()
if command -v pacman >/dev/null 2>&1; then
  pkgs=(git base-devel alsa-lib gtk3 libxkbcommon-x11 libglvnd libcap)
  command -v cargo >/dev/null 2>&1 || pkgs+=(rust)
  while IFS= read -r p; do [ -n "$p" ] && missing+=("$p"); done < <(pacman -T "${pkgs[@]}" 2>/dev/null || true)
  install_pkgs() { sudo pacman -S --needed --noconfirm "$@"; }
elif command -v apt-get >/dev/null 2>&1; then
  for p in git build-essential pkg-config libasound2-dev libgtk-3-dev libxkbcommon-x11-dev libcap2-bin curl; do
    dpkg-query -W -f='${Status}' "$p" 2>/dev/null | grep -q "install ok installed" || missing+=("$p")
  done
  install_pkgs() { sudo apt-get update && sudo apt-get install -y "$@"; }
elif command -v dnf >/dev/null 2>&1; then
  for p in git gcc pkgconf-pkg-config alsa-lib-devel gtk3-devel libxkbcommon-x11-devel libcap curl; do
    rpm -q "$p" >/dev/null 2>&1 || missing+=("$p")
  done
  install_pkgs() { sudo dnf install -y "$@"; }
else
  install_pkgs() { return 1; }
  ui_warn "Unbekannte Distribution: Compiler, pkg-config, ALSA- und GTK3-Entwicklungspakete selbst installieren"
fi
if [ ${#missing[@]} -gt 0 ]; then
  ui_sudo
  ui_run "Pakete installieren (${missing[*]})" install_pkgs "${missing[@]}"
else
  ui_ok "Benötigte Pakete sind da"
fi
if ! command -v cargo >/dev/null 2>&1; then
  ui_run "Rust installieren (einmalig)" sh -c "curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal"
  # shellcheck disable=SC1091
  . "$HOME/.cargo/env"
fi

# The built web UI is committed; restore it if it was deleted locally.
[ -f ui/dist/index.html ] || git checkout HEAD -- ui/dist
V=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)

# --- Bauen (nur Geändertes wird neu übersetzt) -------------------------------
ui_section "Show Master $V"
ui_run "Programm bauen" cargo build --release --locked -p showmaster
BIN=target/release/showmaster

# --- Installieren (nur wenn sich etwas geändert hat) -------------------------
if command -v makepkg >/dev/null 2>&1 && command -v pacman >/dev/null 2>&1; then
  if pacman -Q showmaster 2>/dev/null | grep -q " $V-" && cmp -s /usr/bin/showmaster "$BIN"; then
    ui_skip "Installieren" "Show Master $V ist schon installiert"
  else
    make_pkg() {
      cd packaging/arch && rm -f ./*.pkg.tar.* && SM_VERSION="$V" makepkg -f --noconfirm --nodeps
      rm -rf pkg src
    }
    ui_run "Arch-Paket bauen" make_pkg
    ui_sudo
    ui_run "Installieren" sudo pacman -U --noconfirm packaging/arch/showmaster-"$V"-*.pkg.tar.zst
    rm -f packaging/arch/*.pkg.tar.*
  fi
else
  if cmp -s /usr/local/bin/showmaster "$BIN"; then
    ui_skip "Installieren" "Show Master $V ist schon installiert"
  else
    put() {
      sudo install -Dm755 "$BIN" /usr/local/bin/showmaster
      sudo install -Dm644 packaging/linux/showmaster.desktop /usr/local/share/applications/showmaster.desktop
      sudo install -Dm644 assets/icon.png /usr/local/share/icons/hicolor/512x512/apps/showmaster.png
      # The standard web port 80, so phones only type showmaster.local.
      sudo setcap 'cap_net_bind_service=+ep' /usr/local/bin/showmaster 2>/dev/null || true
    }
    ui_sudo
    ui_run "Installieren nach /usr/local" put
  fi
fi

ui_section "Fertig"
ui_ok "Starten: im Anwendungsmenü oder mit  showmaster"
ui_ok "Am Handy / Tablet im selben Netz:  showmaster.local"
ui_done
