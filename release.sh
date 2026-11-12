#!/bin/bash
# Show Master veröffentlichen - ein Befehl, auf jedem Rechner derselbe:
#
#   ./release.sh          baut, was dieser Rechner bauen kann, und lädt es hoch
#                         (Arch-PC: Linux + Windows, Mac: das DMG)
#   ./release.sh status   zeigt, welche Teile schon oben sind
#
# Die Versionsnummer ist automatisch:
#   * Gibt es einen Entwurf, baut dieser Rechner genau dessen Stand (Git-Tag
#     vX.Y.Z) und ergänzt seine Teile darin.
#   * Ist die aktuelle Version schon veröffentlicht, wird sie um eins erhöht,
#     "was ist neu" aus den Commits seitdem eingetragen, alles gepusht und ein
#     neuer Entwurf angelegt.
# Sobald Mac, Windows und Linux im Entwurf sind, wird er veröffentlicht.
# Nichts wird doppelt gemacht: unveränderte Pakete werden nicht neu gebaut,
# Dateien, die schon oben sind, nicht nochmal hochgeladen.
set -euo pipefail

# --- Einstellungen für dieses Projekt ---------------------------------------
APP="Show Master"
RELEASES="Cheter0410/show-master-releases"
# Pflichtteile: "Name|Datei ({V} = Version)|wer baut es"
PARTS=(
  "Mac|ShowMaster-{V}.dmg|Mac"
  "Windows|ShowMaster-Setup-{V}.exe|Linux-PC"
  "Linux|showmaster-{V}-linux-x86_64.tar.gz|Linux-PC"
)
MAC_BUILD=./packaging/macos/build-dmg.sh
LINUX_BUILD=./packaging/build-on-linux.sh
# Versionsnummer in allen Dateien setzen ($1 = neue Version); perl läuft gleich auf Linux und macOS
set_version() {
  perl -0pi -e "s/^version = \".*?\"/version = \"$1\"/m" Cargo.toml
  perl -0pi -e "s/^  \"version\": \".*?\",/  \"version\": \"$1\",/m" ui/package.json
  perl -0pi -e "s/\A(\{\s*\"name\": \"[^\"]*\",\s*\"version\": \")[^\"]*/\${1}$1/; s/(\"packages\": \{\s*\"\": \{\s*\"name\": \"[^\"]*\",\s*\"version\": \")[^\"]*/\${1}$1/" ui/package-lock.json
  perl -pi -e "s/^pkgver=\\\$\{SM_VERSION:-.*\}/pkgver=\\\${SM_VERSION:-$1}/" packaging/arch/PKGBUILD
}
# ---------------------------------------------------------------------------

# Aus einer Kopie laufen: zum Bauen wird der Stand der Version ausgecheckt,
# und dabei kann sich diese Datei ändern, während Bash sie noch liest.
if [ -z "${RELEASE_SH_COPY:-}" ]; then
  RELEASE_SH_DIR=$(cd "$(dirname "$0")" && pwd)
  copy=$(mktemp)
  cp "$0" "$copy"
  RELEASE_SH_COPY=$copy RELEASE_SH_DIR=$RELEASE_SH_DIR exec bash "$copy" "$@"
fi
trap 'rm -f "$RELEASE_SH_COPY"' EXIT
cd "$RELEASE_SH_DIR"
# shellcheck source=scripts/ui.sh
. scripts/ui.sh
[ "$(uname -s)" = "Darwin" ] && MAC=1 || MAC=
WHO=$([ -n "$MAC" ] && echo Mac || echo Linux-PC)
version() { sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1; }

# --- GitHub-Kommandozeile: installieren, anmelden, Schreibrecht ------------
need_gh() {
  if ! command -v gh >/dev/null 2>&1; then
    if [ -n "$MAC" ]; then
      command -v brew >/dev/null 2>&1 || ui_fail "Homebrew fehlt. Installieren (https://brew.sh), dann nochmal ./release.sh"
      ui_run "GitHub-Kommandozeile installieren" brew install gh
    elif command -v pacman >/dev/null 2>&1; then
      ui_sudo
      ui_run "GitHub-Kommandozeile installieren" sudo pacman -S --needed --noconfirm github-cli
    elif command -v apt-get >/dev/null 2>&1; then
      ui_sudo
      ui_run "GitHub-Kommandozeile installieren" sudo apt-get install -y gh
    else
      ui_fail "Bitte gh installieren (https://cli.github.com), dann nochmal ./release.sh"
    fi
  fi
  if ! gh auth status >/dev/null 2>&1; then
    ui_info "Bei GitHub anmelden (einmalig): gleich öffnet sich der Browser"
    gh auth login --hostname github.com --git-protocol https --web
  fi
  # Git meldet sich über gh an (sonst fragt git pull/push nach Benutzer und
  # Passwort, z. B. wenn bei "gh auth login" SSH gewählt wurde).
  gh auth setup-git --hostname github.com >/dev/null 2>&1 || true
  if [ "$(gh api "repos/$RELEASES" --jq .permissions.push 2>/dev/null || echo false)" != "true" ]; then
    ui_fail "Dein GitHub-Konto darf noch nicht in $RELEASES hochladen.
    Einladung annehmen: https://github.com/$RELEASES/invitations - dann nochmal ./release.sh"
  fi
  ui_ok "GitHub: angemeldet als $(gh api user --jq .login 2>/dev/null || echo ?)"
}

# --- Stand des Releases ----------------------------------------------------
release_state() { # none | draft | published
  local json
  json=$(gh release view "v$1" -R "$RELEASES" --json isDraft 2>/dev/null) || { echo none; return; }
  case "$json" in *'"isDraft":true'*) echo draft ;; *) echo published ;; esac
}
# "name size" je Zeile
release_assets() { gh release view "v$1" -R "$RELEASES" --json assets --jq '.assets[] | "\(.name) \(.size)"' 2>/dev/null || true; }

show_status() {
  local v=$1 state assets missing=0 name file who
  state=$(release_state "$v")
  assets=$(release_assets "$v")
  case "$state" in
    none) ui_section "$APP $v · noch nichts hochgeladen" ;;
    draft) ui_section "$APP $v · Entwurf (für Nutzer noch unsichtbar)" ;;
    published) ui_section "$APP $v · veröffentlicht" ;;
  esac
  for p in "${PARTS[@]}"; do
    IFS='|' read -r name file who <<<"$p"
    file=${file//\{V\}/$v}
    if printf '%s\n' "$assets" | grep -q "^$file "; then
      ui_ok "$(printf '%-8s %s' "$name" "$file")"
    else
      printf '  %s○%s %-8s %sfehlt – kommt vom %s%s\n' "$UI_Y" "$UI_0" "$name" "$UI_D" "$who" "$UI_0"
      missing=1
    fi
  done
  printf '  %s%s%s\n' "$UI_D" "https://github.com/$RELEASES/releases" "$UI_0"
  return $missing
}

# Alle Pflichtteile, die dieser Rechner baut, sind im Release.
mine_complete() {
  local assets name file who
  assets=$(release_assets "$1")
  for p in "${PARTS[@]}"; do
    IFS='|' read -r name file who <<<"$p"
    [ "$who" = "$WHO" ] || continue
    file=${file//\{V\}/$1}
    printf '%s\n' "$assets" | grep -q "^$file " || return 1
  done
  return 0
}

cmd_status() {
  ui_header "$APP" "Release-Status"
  need_gh
  show_status "$(version)" || true
}

# --- Einrichten (nur was fehlt) --------------------------------------------
setup_linux() {
  local need=()
  if [ -z "${SKIP_WINDOWS:-}" ]; then
    command -v x86_64-w64-mingw32-gcc >/dev/null 2>&1 || need+=(mingw-w64-gcc)
    command -v wine >/dev/null 2>&1 || command -v wine64 >/dev/null 2>&1 || need+=(wine)
  fi
  if [ ${#need[@]} -gt 0 ]; then
    command -v pacman >/dev/null 2>&1 || ui_fail "Für den Windows-Bau fehlen: ${need[*]}"
    ui_sudo
    ui_run "Werkzeuge für Windows installieren (${need[*]})" sudo pacman -S --needed --noconfirm "${need[@]}"
  fi
  if [ -z "${SKIP_WINDOWS:-}" ]; then
    rustup target list --installed 2>/dev/null | grep -qx x86_64-pc-windows-gnu ||
      ui_run "Rust-Ziel Windows (einmalig)" rustup target add x86_64-pc-windows-gnu
    local iscc="$HOME/.wine/drive_c/InnoSetup/ISCC.exe"
    if [ ! -f "$iscc" ]; then
      inno() {
        curl -fsSL -o /tmp/innosetup.exe https://github.com/jrsoftware/issrc/releases/download/is-6_7_4/innosetup-6.7.4.exe
        WINEDEBUG=-all WINEDLLOVERRIDES="mscoree,mshtml=" wine /tmp/innosetup.exe /VERYSILENT /SUPPRESSMSGBOXES /NORESTART /CURRENTUSER '/DIR=C:\InnoSetup'
        [ -f "$iscc" ]
      }
      ui_run "Inno Setup einrichten (einmalig)" inno
    fi
  fi
}

setup_mac() {
  if ! xcode-select -p >/dev/null 2>&1; then
    xcode-select --install || true
    ui_fail "Im Fenster, das sich geöffnet hat, auf 'Installieren' klicken; danach nochmal ./release.sh"
  fi
  if ! command -v cargo >/dev/null 2>&1 && [ ! -x "$HOME/.cargo/bin/cargo" ]; then
    ui_run "Rust installieren (einmalig)" sh -c "curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal"
  fi
  # shellcheck disable=SC1091
  [ -f "$HOME/.cargo/env" ] && . "$HOME/.cargo/env"
  return 0
}

# --- Version ---------------------------------------------------------------
changes_since() {
  if git rev-parse -q --verify "refs/tags/$1" >/dev/null; then
    git log --no-merges --format='- %s' "$1..HEAD" | grep -v '^- Version [0-9]' || true
  fi
}

bump_version() {
  local cur=$1 new changes
  IFS=. read -r a b c <<<"$cur"
  new="$a.$b.$((c + 1))"
  changes=$(changes_since "v$cur")
  [ -n "$changes" ] || changes="- Kleinere Verbesserungen"
  set_version "$new"
  awk -v v="$new" -v c="$changes" 'NR==1{print; print ""; print "## " v; print ""; print c; next} {print}' CHANGELOG.md > CHANGELOG.md.new
  mv CHANGELOG.md.new CHANGELOG.md
  cargo update -w >/dev/null 2>&1 || true
  git add -A
  git commit -qm "Version $new"
  git tag "v$new"
  if ! git push -q origin HEAD 2>/dev/null || ! git push -q origin "v$new" 2>/dev/null; then
    # Ein anderer Rechner war gleichzeitig dran: eigene Version zurücknehmen, neu starten.
    git tag -d "v$new" >/dev/null 2>&1 || true
    git reset -q --hard HEAD~1
    ui_info "Ein anderer Rechner war gerade schneller – nehme dessen Version"
    git pull -q --ff-only
    rm -f "$RELEASE_SH_COPY"
    RELEASE_SH_COPY= exec bash "$RELEASE_SH_DIR/release.sh"
  fi
  ui_ok "Neue Version $new"
  printf '%s\n' "$changes" | sed "s/^/      $UI_D/;s/\$/$UI_0/"
}

notes_for() {
  local f
  f=$(mktemp)
  awk -v v="$1" '$0 ~ "^## " v "$" {on=1; next} /^## / {on=0} on' CHANGELOG.md | sed '/./,$!d' > "$f" 2>/dev/null || true
  [ -s "$f" ] || echo "$APP $1" > "$f"
  echo "$f"
}

BACK=""
back_to_branch() { [ -n "$BACK" ] && git checkout -q "$BACK" 2>/dev/null || true; }

# Die Dateien dieses Baus: aus der Liste des Bauskripts, sonst (ältere Stände)
# alles, was beim Bauen neu entstanden ist.
built_files() {
  local list="dist/files-$1.txt" v=$2
  if [ -f "$list" ] && [ "$(head -n 1 "$list")" = "$v" ]; then
    tail -n +2 "$list"
  elif [ -f dist/.build-start ]; then
    find dist -maxdepth 1 -type f -newer dist/.build-start ! -name '.*' ! -name 'files-*.txt' | sort
  fi
}

# --- Release ---------------------------------------------------------------
cmd_release() {
  [ -d .git ] || ui_fail "Das geht nur in einem Git-Checkout (git clone ...)."
  ui_header "$APP" "Release vom $WHO"
  [ -n "$(git status --porcelain --untracked-files=no)" ] && ui_fail "Es gibt lokale Änderungen (git status). Ein Release wird nur aus dem Stand auf GitHub gebaut."

  ui_section "Vorbereiten"
  need_gh
  ui_run "Neuesten Stand holen" sh -c 'git pull -q --ff-only && git fetch -q --tags --force'

  local v state
  v=$(version)
  state=$(release_state "$v")
  case "$state" in
    published)
      if [ -z "$(changes_since "v$v")" ] && git rev-parse -q --verify "refs/tags/v$v" >/dev/null; then
        show_status "$v" || true
        ui_section "Nichts zu tun"
        ui_info "Seit $v hat sich nichts geändert – kein neues Release nötig."
        ui_done
        exit 0
      fi
      bump_version "$v"
      v=$(version)
      ;;
    none | draft)
      git rev-parse -q --verify "refs/tags/v$v" >/dev/null || git tag "v$v"
      # Immer hochschieben: der andere Rechner baut genau diesen Stand.
      git push -q origin "v$v" 2>/dev/null ||
        git ls-remote --exit-code --tags origin "v$v" >/dev/null ||
        ui_fail "Konnte den Tag v$v nicht pushen."
      if [ "$state" = draft ]; then ui_ok "Entwurf $v gefunden – ergänze die Teile vom $WHO"; else ui_ok "Version $v"; fi
      ;;
  esac

  # Sind die Teile dieses Rechners schon im Entwurf, gibt es nichts zu tun
  # (FORCE=1 baut und lädt sie trotzdem neu).
  if [ "$state" = draft ] && [ -z "${FORCE:-}" ] && mine_complete "$v"; then
    ui_ok "Die Teile vom $WHO sind schon im Entwurf – nichts zu bauen"
    if show_status "$v"; then
      ui_run "Veröffentlichen" gh release edit "v$v" -R "$RELEASES" --draft=false --latest
      ui_section "Fertig"
      ui_ok "$APP $v ist veröffentlicht – installierte Programme bieten das Update beim nächsten Start an."
    else
      ui_section "Fertig"
      ui_info "Es fehlen nur noch die mit ○ markierten Teile – die kommen vom anderen Rechner."
    fi
    ui_done
    exit 0
  fi

  # Genau den Stand dieser Version bauen (beide Rechner bauen denselben Code).
  BACK=$(git symbolic-ref -q --short HEAD || git rev-parse HEAD)
  trap 'back_to_branch; rm -f "$RELEASE_SH_COPY"' EXIT
  if [ "$(git rev-parse HEAD)" != "$(git rev-parse "v$v^{commit}")" ]; then
    git checkout -q "v$v"
    ui_info "Baue den Stand von v$v (neuere Änderungen kommen ins nächste Release)"
  fi

  if [ -n "$MAC" ]; then setup_mac; else setup_linux; fi
  local script part
  if [ -n "$MAC" ]; then script=$MAC_BUILD part=mac; else script=$LINUX_BUILD part=linux-pc; fi
  # Neuere Bauskripte zeigen ihre Schritte selbst; ältere Stände leise im Protokoll.
  ui_section "Bauen"
  if grep -q 'scripts/ui.sh' "$script"; then
    "$script"
  else
    mkdir -p dist && touch dist/.build-start
    ui_run "$APP $v bauen" "$script"
  fi

  local files=() f
  while IFS= read -r f; do [ -n "$f" ] && files+=("$f"); done < <(built_files "$part" "$v")
  [ ${#files[@]} -gt 0 ] || ui_fail "Der Bau hat keine Dateien in dist/ erzeugt."

  ui_section "Hochladen"
  if [ "$(release_state "$v")" = "none" ]; then
    local notes
    notes=$(notes_for "$v")
    gh release create "v$v" -R "$RELEASES" --draft --title "$APP $v" --notes-file "$notes" >>"$UI_LOG" 2>&1 ||
      [ "$(release_state "$v")" != none ] || ui_fail "Entwurf konnte nicht angelegt werden (Protokoll: $UI_LOG)"
    rm -f "$notes"
  fi
  # Nur hochladen, was fehlt oder sich geändert hat.
  local assets todo=() size
  assets=$(release_assets "$v")
  for f in "${files[@]}"; do
    size=$(wc -c <"$f" | tr -d ' ')
    if printf '%s\n' "$assets" | grep -qx "$(basename "$f") $size"; then
      ui_skip "$(basename "$f")" "schon oben"
    else
      todo+=("$f")
    fi
  done
  for f in ${todo[@]+"${todo[@]}"}; do
    ui_run "$(basename "$f") ($(du -h "$f" | cut -f1 | tr -d ' '))" gh release upload "v$v" -R "$RELEASES" --clobber "$f"
  done

  if show_status "$v"; then
    ui_run "Veröffentlichen" gh release edit "v$v" -R "$RELEASES" --draft=false --latest
    ui_section "Fertig"
    ui_ok "$APP $v ist veröffentlicht – installierte Programme bieten das Update beim nächsten Start an."
  else
    ui_section "Fertig"
    if [ ${#todo[@]} -gt 0 ]; then
      ui_info "Hochgeladen. Veröffentlicht wird automatisch, sobald die fehlenden Teile oben sind."
    else
      ui_info "Alles von diesem Rechner ist schon oben. Es fehlen nur noch die mit ○ markierten Teile."
    fi
  fi
  ui_done
}

case "${1:-}" in
  "") cmd_release ;;
  status) cmd_status ;;
  -h|--help|help) sed -n '2,16p' "$RELEASE_SH_DIR/release.sh" | sed 's/^# \{0,1\}//' ;;
  *) ui_fail "Unbekannt: $1 (./release.sh oder ./release.sh status)" ;;
esac
