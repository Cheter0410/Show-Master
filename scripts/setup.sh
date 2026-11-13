# Shared steps of the install scripts (source it after scripts/ui.sh, bash 3.2+).
#
#   setup_no_root                 refuse to run as root (the build would belong to root)
#   setup_pull                    newest source, when that is possible without losing anything;
#                                 returns 0 when new commits were pulled (restart the script)
#   setup_rust                    Rust is there and new enough for this project
#   user_app_install ...          Linux: install into the folder Stagehand uses (no sudo)
#   user_app_remove <id> <bin>    Linux: remove the copy in that folder
#   system_copy_remove <bin>      Linux: remove an older copy in /usr/local
#   mac_app_dir / mac_app_put     macOS: the shared folder in Applications, app from a DMG
#
# The same file is used by Show Master, Lantern and Stagehand.

setup_no_root() {
  [ -n "${ALLOW_ROOT:-}" ] && return 0
  [ "$(id -u)" != 0 ] || ui_fail "Bitte ohne sudo starten – das Skript fragt selbst nach dem Passwort, wenn es eins braucht."
}

# Newest source. Never fails: without internet, with local changes or with a
# history that cannot simply be moved forward, the state that is here is built.
setup_pull() {
  if [ ! -d .git ] || ! command -v git >/dev/null 2>&1; then
    ui_ok "Quellcode: der Stand, der hier liegt (kein Git-Checkout)"
    return 1
  fi
  if ! git diff --quiet 2>/dev/null || ! git diff --cached --quiet 2>/dev/null; then
    ui_warn "Lokale Änderungen – baue den Stand, der hier liegt"
    return 1
  fi
  if ! git rev-parse -q --verify '@{u}' >/dev/null 2>&1; then
    ui_ok "Quellcode: der Stand, der hier liegt (Branch ohne GitHub-Gegenstück)"
    return 1
  fi
  if ! git fetch -q 2>>"$UI_LOG"; then
    ui_warn "GitHub nicht erreichbar – baue den Stand, der hier liegt"
    return 1
  fi
  if [ -z "$(git rev-list 'HEAD..@{u}' 2>/dev/null)" ]; then
    ui_ok "Quellcode ist aktuell"
    return 1
  fi
  if git merge -q --ff-only '@{u}' >>"$UI_LOG" 2>&1; then
    ui_ok "Neuesten Stand geholt"
    return 0
  fi
  ui_warn "Hier gibt es eigene Commits, die nicht zu GitHub passen – baue den Stand, der hier liegt"
  return 1
}

# "1.85" from Cargo.toml (rust-version), empty when it names none.
setup_rust_wanted() { sed -n 's/^rust-version *= *"\(.*\)"/\1/p' Cargo.toml | head -n 1; }

# 0 when version $1 >= $2 (numeric, dotted).
setup_version_ge() {
  local a b i x y
  IFS=. read -r -a a <<<"$1"
  IFS=. read -r -a b <<<"$2"
  for i in 0 1 2; do
    x=${a[$i]:-0} y=${b[$i]:-0}
    x=${x%%[!0-9]*} y=${y%%[!0-9]*}
    [ "${x:-0}" -gt "${y:-0}" ] && return 0
    [ "${x:-0}" -lt "${y:-0}" ] && return 1
  done
  return 0
}

setup_rust() {
  # A Rust installed earlier may not be on this shell's PATH (fish, new terminal).
  # shellcheck disable=SC1091
  [ -f "$HOME/.cargo/env" ] && . "$HOME/.cargo/env"
  local want have
  want=$(setup_rust_wanted)
  if ! command -v cargo >/dev/null 2>&1; then
    command -v curl >/dev/null 2>&1 || ui_fail "Rust fehlt und curl auch. Rust installieren (https://rustup.rs), dann nochmal."
    ui_run "Rust installieren (einmalig)" sh -c "curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal"
    # shellcheck disable=SC1091
    . "$HOME/.cargo/env"
  fi
  have=$(rustc --version 2>/dev/null | sed -n 's/^rustc \([0-9.]*\).*/\1/p' || true)
  if [ -n "$want" ] && [ -n "$have" ] && ! setup_version_ge "$have" "$want"; then
    if command -v rustup >/dev/null 2>&1; then
      ui_run "Rust aktualisieren ($have → mindestens $want)" rustup update stable
    elif command -v pacman >/dev/null 2>&1; then
      ui_sudo
      ui_run "Rust aktualisieren ($have → mindestens $want)" sudo pacman -S --noconfirm rust
    else
      # The distribution's Rust is too old: rustup's own goes in front of it.
      ui_run "Neueres Rust installieren ($have → mindestens $want)" sh -c "curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal"
      # shellcheck disable=SC1091
      . "$HOME/.cargo/env"
      export PATH="$HOME/.cargo/bin:$PATH"
    fi
    have=$(rustc --version 2>/dev/null | sed -n 's/^rustc \([0-9.]*\).*/\1/p' || true)
    setup_version_ge "${have:-0}" "$want" || ui_fail "Rust $have ist zu alt (gebraucht: $want). Rust aktualisieren, dann nochmal."
  fi
  ui_ok "Rust ${have:-?}"
}

# --- Linux: the folder all show tools share (as Stagehand installs them) ----
USER_APPS="${XDG_DATA_HOME:-$HOME/.local/share}/stagehand/apps"
USER_MENU="${XDG_DATA_HOME:-$HOME/.local/share}/applications"
USER_BIN="$HOME/.local/bin"

# user_app_current <id> <bin> <program file>: that very program is installed.
user_app_current() { cmp -s "$USER_APPS/$1/$2" "$3"; }

# user_app_install <id> <bin> <version> <program file> <desktop file> <icon>
# No sudo. The folder is swapped in one move, so a half copy never stays.
user_app_install() {
  local id=$1 bin=$2 version=$3 prog=$4 desktop=$5 icon=$6 new dir
  dir="$USER_APPS/$id"
  new="$USER_APPS/.$id.new"
  mkdir -p "$USER_APPS" "$USER_MENU" "$USER_BIN"
  rm -rf "$new"
  mkdir -p "$new"
  install -m755 "$prog" "$new/$bin"
  install -m644 "$icon" "$new/icon.png"
  [ -f README.md ] && install -m644 README.md "$new/README.md"
  printf '%s\n' "$version" >"$new/VERSION"
  rm -rf "$dir"
  mv "$new" "$dir"
  # Menu entry with full paths (works without ~/.local/bin on the PATH).
  local args
  args=$(sed -n 's/^Exec=[^ ]*//p' "$desktop" | head -n 1)
  {
    grep -vE '^(Exec|Icon|X-Stagehand)=' "$desktop"
    printf 'Exec="%s"%s\nIcon=%s\nX-Stagehand=1\n' "$dir/$bin" "$args" "$dir/icon.png"
  } >"$USER_MENU/$bin.desktop"
  # The command; a real file someone put there is left alone.
  if [ -L "$USER_BIN/$bin" ] || [ ! -e "$USER_BIN/$bin" ]; then ln -sfn "$dir/$bin" "$USER_BIN/$bin"; fi
  command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database -q "$USER_MENU" 2>/dev/null
  return 0
}

# user_app_remove <id> <bin>: the copy Stagehand (or an install script) put
# into the shared folder, with its menu entry and command.
user_app_remove() {
  local id=$1 bin=$2 dir="$USER_APPS/$1"
  [ -e "$dir" ] || return 1
  rm -rf "$dir"
  if grep -qs '^X-Stagehand=1' "$USER_MENU/$bin.desktop"; then rm -f "$USER_MENU/$bin.desktop"; fi
  case "$(readlink "$USER_BIN/$bin" 2>/dev/null)" in "$dir"/*) rm -f "$USER_BIN/$bin" ;; esac
  return 0
}

# system_copy_there <bin>: an older install script put a copy into /usr/local.
system_copy_there() { [ -e "/usr/local/bin/$1" ] || [ -e "/usr/local/share/applications/$1.desktop" ]; }
system_copy_remove() {
  sudo rm -f "/usr/local/bin/$1" "/usr/local/share/applications/$1.desktop" \
    "/usr/local/share/icons/hicolor/512x512/apps/$1.png"
}

# ~/.local/bin on the PATH, so the command works in a terminal too.
user_bin_on_path() {
  case ":$PATH:" in *":$USER_BIN:"*) return 0 ;; esac
  if command -v fish >/dev/null 2>&1; then fish -c "fish_add_path '$USER_BIN'" >/dev/null 2>&1 || true; fi
  ui_info "Im Terminal: $USER_BIN ist noch nicht im PATH (das Anwendungsmenü geht trotzdem)"
}

# --- macOS: the folder all show tools share (as Stagehand installs them) ----
# /Applications/Stagehand, or the one in the home folder when this user may
# not write to /Applications (no admin account).
mac_app_dir() {
  local d=/Applications/Stagehand
  if mkdir -p "$d" 2>/dev/null && [ -w "$d" ]; then
    echo "$d"
  else
    mkdir -p "$HOME/Applications/Stagehand"
    echo "$HOME/Applications/Stagehand"
  fi
}

# mac_app_put <dmg> <Name.app> <folder>: the app from the DMG into the folder.
# Copied next to the old one first, so a failed copy leaves the old app.
mac_app_put() {
  local dmg=$1 app=$2 dir=$3 mnt ok=0
  mnt=$(mktemp -d)
  hdiutil attach -nobrowse -readonly -mountpoint "$mnt" "$dmg" >/dev/null
  rm -rf "$dir/.$app.new"
  if ditto "$mnt/$app" "$dir/.$app.new"; then ok=1; fi
  hdiutil detach "$mnt" -quiet >/dev/null 2>&1 || true
  rmdir "$mnt" 2>/dev/null || true
  if [ "$ok" != 1 ]; then rm -rf "$dir/.$app.new"; return 1; fi
  rm -rf "$dir/$app"
  mv "$dir/.$app.new" "$dir/$app"
  xattr -dr com.apple.quarantine "$dir/$app" 2>/dev/null || true
  # A loose copy from before the shared folder would be started by Spotlight too.
  local old
  for old in "/Applications/$app" "$HOME/Applications/$app"; do
    [ -d "$old" ] && rm -rf "$old" 2>/dev/null
  done
  return 0
}
