#!/bin/bash
# Upload what this computer just built (dist/) to the GitHub release
# v<version> in Cheter0410/show-master-releases - the in-app updater looks
# there. Creates the release when it does not exist yet, otherwise adds the
# files (replacing ones with the same name), so the Arch PC (Linux + Windows)
# and the Mac (DMG) can upload to the same release in any order.
#
#   ./packaging/publish.sh          asks first
#   PUBLISH=1 ./packaging/publish.sh   no question
#
# Called at the end of packaging/build-on-linux.sh and install-mac.sh.
# Needs the GitHub command line, logged in once: `gh auth login`.
set -euo pipefail
cd "$(dirname "$0")/.."
REPO="${RELEASE_REPO:-Cheter0410/show-master-releases}"
V=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)

[ "${PUBLISH:-}" = "0" ] && exit 0
if ! command -v gh >/dev/null 2>&1; then
  echo "Not uploaded: the GitHub command line (gh) is not installed."
  echo "  Arch: sudo pacman -S github-cli   Mac: brew install gh   then once: gh auth login"
  echo "  Afterwards: ./packaging/publish.sh"
  exit 0
fi
if ! gh auth status >/dev/null 2>&1; then
  echo "Not uploaded: gh is not logged in. Run once: gh auth login - then ./packaging/publish.sh"
  exit 0
fi

# Only files from the last build: newer than its start mark (old builds
# of other versions may still lie in dist/).
FILES=()
if [ -f dist/.build-start ]; then
  while IFS= read -r f; do FILES+=("$f"); done < <(find dist -maxdepth 1 -type f -newer dist/.build-start ! -name '.*' | sort)
else
  while IFS= read -r f; do FILES+=("$f"); done < <(find dist -maxdepth 1 -type f -name "*$V*" | sort)
fi
if [ ${#FILES[@]} -eq 0 ]; then
  echo "Nothing to upload: no files from a build of $V in dist/."
  exit 0
fi

if [ -n "$(git status --porcelain --untracked-files=no 2>/dev/null)" ]; then
  echo "Note: this checkout has local changes - the files may not match version $V on GitHub."
fi
echo
echo "Upload to release v$V of $REPO:"
printf '  %s\n' "${FILES[@]}"
if [ "${PUBLISH:-}" != "1" ]; then
  if [ ! -t 0 ]; then
    echo "(not asked - no terminal; PUBLISH=1 uploads without asking)"
    exit 0
  fi
  read -r -p "Upload now? [y/N] " answer
  case "$answer" in y|Y|j|J|yes|ja) ;; *) echo "Not uploaded (later: ./packaging/publish.sh)."; exit 0 ;; esac
fi

NOTES=$(mktemp)
awk -v v="$V" '$0 ~ "^## " v "$" {on=1; next} /^## / {on=0} on' CHANGELOG.md > "$NOTES" 2>/dev/null || true
[ -s "$NOTES" ] || echo "Show Master $V" > "$NOTES"
if gh release view "v$V" -R "$REPO" >/dev/null 2>&1; then
  gh release upload "v$V" -R "$REPO" --clobber "${FILES[@]}"
else
  gh release create "v$V" -R "$REPO" --title "Show Master $V" --notes-file "$NOTES" "${FILES[@]}"
fi
rm -f "$NOTES"
echo "Uploaded. Installed copies of Show Master offer the update the next time they check."
