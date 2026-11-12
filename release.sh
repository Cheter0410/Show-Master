#!/bin/bash
# Show Master veröffentlichen - ein Befehl, auf jedem Rechner derselbe:
#
#   ./release.sh          baut, was dieser Rechner bauen kann, und lädt es hoch
#                         (Arch-PC: Linux + Windows, Mac: das DMG)
#   ./release.sh status   zeigt, welche Teile schon oben sind
#
# Die Versionsnummer ist automatisch:
#   * Gibt es einen Entwurf, baut dieser Rechner genau dessen Stand (Git-Tag
#     vX.Y.Z) und ersetzt seine Teile darin.
#   * Ist die aktuelle Version schon veröffentlicht, wird sie um eins erhöht,
#     "was ist neu" aus den Commits seitdem eingetragen, alles gepusht und ein
#     neuer Entwurf angelegt.
# Sobald Mac, Windows und Linux im Entwurf sind, wird er veröffentlicht -
# erst dann bieten installierte Programme das Update an.
set -euo pipefail
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
LOG="${TMPDIR:-/tmp}/release-$(basename "$(pwd)").log"
# Einen Bauschritt leise ausführen: alles ins Protokoll, auf dem Bildschirm
# nur die Schritte ("== ...") und Ergebnisse. Bei einem Fehler: die letzten
# Zeilen und wo das ganze Protokoll liegt.
quiet() {
  local rc
  set +e
  "$@" 2>&1 | tee -a "$LOG" | grep --line-buffered -E '^== |^Tests ok|^Built |^Wine or Inno' | while IFS= read -r l; do printf '   · %s\n' "${l#== }"; done
  rc=${PIPESTATUS[0]}
  set -e
  if [ "$rc" -ne 0 ]; then
    printf '\n\033[31mFehlgeschlagen. Die letzten Zeilen:\033[0m\n'
    tail -n 30 "$LOG"
    printf '\nGanzes Protokoll: %s\n' "$LOG"
    exit 1
  fi
}
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
  # Git meldet sich über gh an (sonst fragt git pull/push nach Benutzer und
  # Passwort, z. B. wenn bei "gh auth login" SSH gewählt wurde).
  gh auth setup-git --hostname github.com >/dev/null 2>&1 || true
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

cmd_status() {
  need_gh
  show_status "$(version)" || true
}

setup_linux() {
  # Windows-Bau unter Linux: MinGW, Wine und Inno Setup (einmalig)
  local need=()
  command -v x86_64-w64-mingw32-gcc >/dev/null 2>&1 || need+=(mingw-w64-gcc)
  command -v wine >/dev/null 2>&1 || command -v wine64 >/dev/null 2>&1 || need+=(wine)
  if [ ${#need[@]} -gt 0 ]; then
    command -v pacman >/dev/null 2>&1 || fail "Für den Windows-Bau fehlen: ${need[*]} (siehe docs/release-anleitung.md)"
    step "Werkzeuge für den Windows-Bau installieren (einmalig)"
    sudo pacman -S --needed --noconfirm "${need[@]}" >>"$LOG" 2>&1 || fail "Konnte ${need[*]} nicht installieren. Für wine muss [multilib] in /etc/pacman.conf aktiv sein."
  fi
  rustup target add x86_64-pc-windows-gnu >>"$LOG" 2>&1
  local iscc="$HOME/.wine/drive_c/InnoSetup/ISCC.exe"
  if [ ! -f "$iscc" ]; then
    step "Inno Setup (baut den Windows-Installer) einrichten (einmalig)"
    curl -fsSL -o /tmp/innosetup.exe https://github.com/jrsoftware/issrc/releases/download/is-6_7_4/innosetup-6.7.4.exe
    WINEDEBUG=-all WINEDLLOVERRIDES="mscoree,mshtml=" wine /tmp/innosetup.exe /VERYSILENT /SUPPRESSMSGBOXES /NORESTART /CURRENTUSER '/DIR=C:\InnoSetup' >>"$LOG" 2>&1
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
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal >>"$LOG" 2>&1
  fi
  # shellcheck disable=SC1091
  [ -f "$HOME/.cargo/env" ] && . "$HOME/.cargo/env"
}

# Was neu ist: die Commits seit dem letzten veröffentlichten Stand.
changes_since() {
  local from=$1
  if git rev-parse -q --verify "refs/tags/$from" >/dev/null; then
    git log --no-merges --format='- %s' "$from..HEAD" | grep -v '^- Version [0-9]' || true
  fi
}

# Version erhöhen, "was ist neu" eintragen, committen, taggen, pushen.
# Ist ein anderer Rechner schneller gewesen, fängt das Skript von vorn an.
bump_version() {
  local cur=$1 new changes
  IFS=. read -r a b c <<<"$cur"
  new="$a.$b.$((c + 1))"
  changes=$(changes_since "v$cur")
  [ -n "$changes" ] || changes="- Kleinere Verbesserungen"
  step "Neue Version $new"
  printf '%s\n' "$changes"
  set_version "$new"
  awk -v v="$new" -v c="$changes" 'NR==1{print; print ""; print "## " v; print ""; print c; next} {print}' CHANGELOG.md > CHANGELOG.md.new
  mv CHANGELOG.md.new CHANGELOG.md
  cargo update -w >/dev/null 2>&1 || true
  git add -A
  git commit -qm "Version $new"
  git tag "v$new"
  if ! git push -q origin HEAD 2>/dev/null || ! git push -q origin "v$new" 2>/dev/null; then
    # Jemand anderes hat gleichzeitig eine Version angelegt: eigene zurücknehmen, neu starten.
    git tag -d "v$new" >/dev/null 2>&1 || true
    git reset -q --hard HEAD~1
    echo "Ein anderer Rechner war gerade schneller - nehme dessen Version."
    git pull -q --ff-only
    rm -f "$RELEASE_SH_COPY"
    RELEASE_SH_COPY= exec bash "$RELEASE_SH_DIR/release.sh" "$@"
  fi
}

notes_for() {
  local v=$1 f
  f=$(mktemp)
  awk -v v="$v" '$0 ~ "^## " v "$" {on=1; next} /^## / {on=0} on' CHANGELOG.md > "$f" 2>/dev/null || true
  [ -s "$f" ] || echo "$APP $v" > "$f"
  echo "$f"
}

BACK=""
back_to_branch() { [ -n "$BACK" ] && git checkout -q "$BACK" 2>/dev/null || true; }

cmd_release() {
  : > "$LOG"
  [ -d .git ] || fail "Das geht nur in einem Git-Checkout (git clone ...)."
  [ -n "$(git status --porcelain --untracked-files=no)" ] && fail "Es gibt lokale Änderungen (git status). Ein Release wird nur aus dem Stand auf GitHub gebaut."
  # Erst anmelden: schon der Pull braucht bei privaten Repos die Anmeldung.
  need_gh
  step "Neuesten Stand holen"
  git pull -q --ff-only || fail "Konnte den neuesten Stand nicht holen."
  git fetch -q --tags --force

  local v state
  v=$(version)
  state=$(release_state "$v")
  case "$state" in
    published)
      if [ -z "$(changes_since "v$v")" ] && git rev-parse -q --verify "refs/tags/v$v" >/dev/null; then
        show_status "$v" || true
        bold "Seit $v hat sich nichts geändert - kein neues Release nötig."
        exit 0
      fi
      bump_version "$v" "$@"
      v=$(version)
      ;;
    none | draft)
      # Erste Veröffentlichung dieser Version, oder ein Entwurf wartet: Tag setzen, falls er fehlt.
      if ! git rev-parse -q --verify "refs/tags/v$v" >/dev/null; then
        git tag "v$v"
      fi
      # Immer hochschieben: der andere Rechner baut genau diesen Stand
      # (auch wenn ein abgebrochener Lauf den Tag nur lokal angelegt hat).
      git push -q origin "v$v" 2>/dev/null \
        || git ls-remote --exit-code --tags origin "v$v" >/dev/null \
        || fail "Konnte den Tag v$v nicht pushen."
      ;;
  esac

  # Genau den Stand dieser Version bauen (beide Rechner bauen denselben Code).
  BACK=$(git symbolic-ref -q --short HEAD || git rev-parse HEAD)
  trap 'back_to_branch; rm -f "$RELEASE_SH_COPY"' EXIT
  if [ "$(git rev-parse HEAD)" != "$(git rev-parse "v$v^{commit}")" ]; then
    echo "Baue den Stand von v$v (nicht die neueren Änderungen danach)."
    git checkout -q "v$v"
  fi

  if [ -n "$MAC" ]; then
    setup_mac
    step "$APP $v für den Mac bauen (dauert beim ersten Mal ein paar Minuten)"
    quiet build_mac
  else
    setup_linux
    step "$APP $v für Linux und Windows bauen (dauert beim ersten Mal ein paar Minuten)"
    quiet build_linux
  fi

  local files=()
  while IFS= read -r f; do files+=("$f"); done < <(find dist -maxdepth 1 -type f -newer dist/.build-start ! -name '.*' | sort)
  [ ${#files[@]} -gt 0 ] || fail "Der Bau hat keine Dateien in dist/ erzeugt."

  step "Hochladen"
  if [ "$(release_state "$v")" = "none" ]; then
    local notes
    notes=$(notes_for "$v")
    gh release create "v$v" -R "$RELEASES" --draft --title "$APP $v" --notes-file "$notes" "${files[@]}" >>"$LOG" 2>&1 \
      || gh release upload "v$v" -R "$RELEASES" --clobber "${files[@]}" >>"$LOG" 2>&1 \
      || fail "Hochladen fehlgeschlagen (Protokoll: $LOG)"
    rm -f "$notes"
  else
    gh release upload "v$v" -R "$RELEASES" --clobber "${files[@]}" >>"$LOG" 2>&1 \
      || fail "Hochladen fehlgeschlagen (Protokoll: $LOG)"
  fi
  printf '   · %s\n' "${files[@]##*/}"

  if show_status "$v"; then
    step "Alles da - veröffentlichen"
    gh release edit "v$v" -R "$RELEASES" --draft=false --latest >>"$LOG" 2>&1
    bold "$APP $v ist veröffentlicht. Installierte Programme bieten das Update beim nächsten Start an."
  else
    bold "Hochgeladen. Veröffentlicht wird automatisch, sobald die fehlenden Teile oben sind."
  fi
}

case "${1:-}" in
  "") cmd_release "$@" ;;
  status) cmd_status ;;
  -h|--help|help) sed -n '2,16p' "$RELEASE_SH_DIR/release.sh" | sed 's/^# \{0,1\}//' ;;
  *) fail "Unbekannt: $1 (./release.sh oder ./release.sh status)" ;;
esac
