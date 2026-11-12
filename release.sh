#!/bin/bash
# Show Master veröffentlichen - ein Befehl, auf jedem Rechner derselbe.
#
#   ./release.sh            baut, was dieser Rechner bauen kann, und lädt es hoch
#                           (Arch-PC: Linux + Windows, Mac: das DMG)
#   ./release.sh status     zeigt, welche Teile der aktuellen Version schon oben sind
#   ./release.sh version    erhöht die Versionsnummer (danach bauen beide neu)
#
# Hochgeladen wird in einen Entwurf. Sobald Mac, Windows und Linux drin sind,
# wird das Release automatisch veröffentlicht - erst dann bieten installierte
# Show Master das Update an. Fehlende Werkzeuge richtet das Skript selbst ein.
set -euo pipefail
cd "$(dirname "$0")"

# --- Einstellungen für dieses Projekt ---------------------------------------
APP="Show Master"
RELEASES="Cheter0410/show-master-releases"
# Pflichtteile: "Name|Datei ({V} = Version)|wer baut es"
PARTS=(
  "Mac|ShowMaster-{V}.dmg|Mac"
  "Windows|ShowMaster-Setup-{V}.exe|Linux-PC"
  "Linux|showmaster-{V}-linux-x86_64.tar.gz|Linux-PC"
)
build_mac()   { NO_INSTALL=1 PUBLISH=0 ./packaging/macos/build-dmg.sh; }
build_linux() { PUBLISH=0 ./packaging/build-on-linux.sh; }
# Versionsnummer in allen Dateien setzen ($1 = neue Version)
set_version() {
  # perl statt sed: läuft gleich auf Linux und macOS
  perl -0pi -e "s/^version = \".*?\"/version = \"$1\"/m" Cargo.toml
  perl -0pi -e "s/^  \"version\": \".*?\",/  \"version\": \"$1\",/m" ui/package.json
  perl -0pi -e "s/\A(\{\s*\"name\": \"[^\"]*\",\s*\"version\": \")[^\"]*/\${1}$1/; s/(\"packages\": \{\s*\"\": \{\s*\"name\": \"[^\"]*\",\s*\"version\": \")[^\"]*/\${1}$1/" ui/package-lock.json
  perl -pi -e "s/^pkgver=\\\$\{SM_VERSION:-.*\}/pkgver=\\\${SM_VERSION:-$1}/" packaging/arch/PKGBUILD
}
# ---------------------------------------------------------------------------

bold() { printf '\033[1m%s\033[0m\n' "$*"; }
step() { printf '\n\033[1m== %s\033[0m\n' "$*"; }
fail() { printf '\n\033[31m%s\033[0m\n' "$*"; exit 1; }
[ "$(uname -s)" = "Darwin" ] && MAC=1 || MAC=
version() { sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1; }

# --- GitHub-Kommandozeile: installieren, anmelden, Schreibrecht prüfen -----
need_gh() {
  if ! command -v gh >/dev/null 2>&1; then
    step "GitHub-Kommandozeile (gh) installieren"
    if [ -n "$MAC" ]; then
      command -v brew >/dev/null 2>&1 || fail "Homebrew fehlt. Installieren (https://brew.sh), dann nochmal ./release.sh"
      brew install gh
    elif command -v pacman >/dev/null 2>&1; then
      sudo pacman -S --needed --noconfirm github-cli
    elif command -v apt-get >/dev/null 2>&1; then
      sudo apt-get install -y gh
    else
      fail "Bitte gh installieren (https://cli.github.com), dann nochmal ./release.sh"
    fi
  fi
  if ! gh auth status >/dev/null 2>&1; then
    step "Bei GitHub anmelden (einmalig)"
    echo "Gleich öffnet sich der Browser: mit deinem GitHub-Konto anmelden und den Code bestätigen."
    gh auth login --hostname github.com --git-protocol https --web
  fi
  if [ "$(gh api "repos/$RELEASES" --jq .permissions.push 2>/dev/null || echo false)" != "true" ]; then
    fail "Dein GitHub-Konto darf noch nicht in $RELEASES hochladen.
Die Einladung annehmen: https://github.com/$RELEASES/invitations
(oder den Besitzer bitten: Settings > Collaborators > Add people). Dann nochmal ./release.sh"
  fi
}

# --- Stand des Releases ----------------------------------------------------
# state: none | draft | published
release_state() {
  local v=$1 json
  json=$(gh release view "v$v" -R "$RELEASES" --json isDraft 2>/dev/null) || { echo none; return; }
  case "$json" in *'"isDraft":true'*) echo draft ;; *) echo published ;; esac
}
release_files() { gh release view "v$1" -R "$RELEASES" --json assets --jq '.assets[].name' 2>/dev/null || true; }

show_status() {
  local v=$1 state files missing=0
  state=$(release_state "$v")
  files=$(release_files "$v")
  echo
  case "$state" in
    none) bold "$APP $v: noch nichts hochgeladen" ;;
    draft) bold "$APP $v: Entwurf (noch nicht für Nutzer sichtbar)" ;;
    published) bold "$APP $v: veröffentlicht" ;;
  esac
  for p in "${PARTS[@]}"; do
    IFS='|' read -r name file who <<<"$p"
    file=${file//\{V\}/$v}
    if grep -qxF "$file" <<<"$files"; then
      printf '  \033[32m✓\033[0m %-8s %s\n' "$name" "$file"
    else
      printf '  \033[33m·\033[0m %-8s fehlt noch - baut der %s mit ./release.sh\n' "$name" "$who"
      missing=1
    fi
  done
  echo "  https://github.com/$RELEASES/releases"
  return $missing
}

notes_file() {
  local v=$1 f
  f=$(mktemp)
  awk -v v="$v" '$0 ~ "^## " v "$" {on=1; next} /^## / {on=0} on' CHANGELOG.md > "$f" 2>/dev/null || true
  [ -s "$f" ] || echo "$APP $v" > "$f"
  echo "$f"
}

# --- Befehle ----------------------------------------------------------------
cmd_status() {
  need_gh
  show_status "$(version)" || true
}

cmd_version() {
  local cur new
  cur=$(version)
  IFS=. read -r a b c <<<"$cur"
  new=${1:-$a.$b.$((c + 1))}
  [ -n "$(git status --porcelain --untracked-files=no)" ] && fail "Erst die offenen Änderungen committen (git status)."
  git pull --ff-only || true
  echo "Version $cur -> $new"
  set_version "$new"
  if ! grep -q "^## $new\$" CHANGELOG.md; then
    awk -v v="$new" 'NR==1{print; print ""; print "## " v; print ""; print "- (was ist neu? hier eintragen)"; next} {print}' CHANGELOG.md > CHANGELOG.md.new
    mv CHANGELOG.md.new CHANGELOG.md
  fi
  if [ -t 0 ]; then
    echo "Jetzt kurz in CHANGELOG.md eintragen, was neu ist (das sehen die Nutzer im Update-Dialog)."
    "${EDITOR:-nano}" CHANGELOG.md || true
  fi
  cargo update -w >/dev/null 2>&1 || true
  git add -A && git commit -qm "Version $new" && git push -q
  bold "Version $new ist eingetragen und gepusht. Jetzt auf jedem Rechner: ./release.sh"
}

setup_linux() {
  # Windows-Bau unter Linux: MinGW, Wine und Inno Setup (einmalig)
  local need=()
  command -v x86_64-w64-mingw32-gcc >/dev/null 2>&1 || need+=(mingw-w64-gcc)
  command -v wine >/dev/null 2>&1 || command -v wine64 >/dev/null 2>&1 || need+=(wine)
  if [ ${#need[@]} -gt 0 ]; then
    command -v pacman >/dev/null 2>&1 || fail "Für den Windows-Bau fehlen: ${need[*]} (siehe docs/release-anleitung.md)"
    step "Werkzeuge für den Windows-Bau installieren (einmalig)"
    sudo pacman -S --needed --noconfirm "${need[@]}" || fail "Konnte ${need[*]} nicht installieren. Für wine muss [multilib] in /etc/pacman.conf aktiv sein."
  fi
  rustup target add x86_64-pc-windows-gnu >/dev/null
  local iscc="$HOME/.wine/drive_c/InnoSetup/ISCC.exe"
  if [ ! -f "$iscc" ]; then
    step "Inno Setup (baut den Windows-Installer) einrichten (einmalig)"
    curl -fL -o /tmp/innosetup.exe https://github.com/jrsoftware/issrc/releases/download/is-6_7_4/innosetup-6.7.4.exe
    WINEDEBUG=-all WINEDLLOVERRIDES="mscoree,mshtml=" wine /tmp/innosetup.exe /VERYSILENT /SUPPRESSMSGBOXES /NORESTART /CURRENTUSER '/DIR=C:\InnoSetup'
    [ -f "$iscc" ] || fail "Inno Setup ließ sich nicht einrichten (siehe docs/release-anleitung.md)."
  fi
}

setup_mac() {
  if ! xcode-select -p >/dev/null 2>&1; then
    xcode-select --install || true
    fail "Im Fenster, das sich geöffnet hat, auf 'Installieren' klicken; danach nochmal ./release.sh"
  fi
  if ! command -v cargo >/dev/null 2>&1 && [ ! -x "$HOME/.cargo/bin/cargo" ]; then
    step "Rust installieren (einmalig)"
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal
  fi
  # shellcheck disable=SC1091
  [ -f "$HOME/.cargo/env" ] && . "$HOME/.cargo/env"
}

cmd_release() {
  if [ -d .git ]; then
    [ -n "$(git status --porcelain --untracked-files=no)" ] && fail "Es gibt lokale Änderungen (git status). Ein Release wird nur aus dem Stand auf GitHub gebaut."
    step "Neuesten Stand holen"
    git pull --ff-only || fail "Konnte den neuesten Stand nicht holen."
  fi
  need_gh
  local v state
  v=$(version)
  state=$(release_state "$v")
  if [ "$state" = "published" ]; then
    show_status "$v" || true
    fail "$APP $v ist schon veröffentlicht. Für ein neues Release erst die Version erhöhen: ./release.sh version"
  fi

  if [ -n "$MAC" ]; then
    setup_mac
    step "$APP $v für den Mac bauen (dauert beim ersten Mal ein paar Minuten)"
    build_mac
  else
    setup_linux
    step "$APP $v für Linux und Windows bauen (dauert beim ersten Mal ein paar Minuten)"
    build_linux
  fi

  # Die Dateien dieses Baus (neuer als die Startmarke des Bauskripts)
  local files=()
  while IFS= read -r f; do files+=("$f"); done < <(find dist -maxdepth 1 -type f -newer dist/.build-start ! -name '.*' | sort)
  [ ${#files[@]} -gt 0 ] || fail "Der Bau hat keine Dateien in dist/ erzeugt."

  step "Hochladen"
  if [ "$(release_state "$v")" = "none" ]; then
    local notes
    notes=$(notes_file "$v")
    gh release create "v$v" -R "$RELEASES" --draft --title "$APP $v" --notes-file "$notes" "${files[@]}"
    rm -f "$notes"
  else
    gh release upload "v$v" -R "$RELEASES" --clobber "${files[@]}"
  fi

  if show_status "$v"; then
    step "Alles da - veröffentlichen"
    gh release edit "v$v" -R "$RELEASES" --draft=false --latest
    bold "$APP $v ist veröffentlicht. Installierte Programme bieten das Update beim nächsten Start an."
  else
    bold "Hochgeladen. Veröffentlicht wird automatisch, sobald die fehlenden Teile oben sind."
  fi
}

case "${1:-}" in
  "") cmd_release ;;
  status) cmd_status ;;
  version) shift; cmd_version "${1:-}" ;;
  -h|--help|help) sed -n '2,12p' "$0" | sed 's/^# \{0,1\}//' ;;
  *) fail "Unbekannt: $1 (./release.sh, ./release.sh status, ./release.sh version)" ;;
esac
