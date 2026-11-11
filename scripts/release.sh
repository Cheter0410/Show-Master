#!/bin/sh
# Builds the installers on GitHub for the version in Cargo.toml and publishes them to the
# public downloads repository that the in-app updater reads (needs the `gh` CLI, logged in).
# Safe to run again for the same version: it continues where it left off.
set -eu
cd "$(dirname "$0")/.."
PUBLIC=Cheter0410/show-master-releases
V=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)
TAG="v$V"
if ! git rev-parse -q --verify "refs/tags/$TAG" >/dev/null; then
  if [ -n "$(git status --porcelain)" ]; then echo "Commit your changes first."; exit 1; fi
  git tag "$TAG"
  git push origin "$TAG"
fi
echo "Waiting for the build of $TAG to start..."
RUN=""
for _ in $(seq 1 30); do
  RUN=$(gh run list --workflow build --branch "$TAG" --limit 1 --json databaseId -q '.[0].databaseId' 2>/dev/null || true)
  [ -n "$RUN" ] && break
  sleep 5
done
[ -n "$RUN" ] || { echo "No build run found for $TAG"; exit 1; }
gh run watch "$RUN" --exit-status
NOTES=$(awk -v v="$V" '$0 ~ "^## " v "$" {on=1; next} /^## / {on=0} on' CHANGELOG.md | sed -e '/./,$!d')
[ -n "$NOTES" ] || NOTES="Show Master $V"
OUT=$(mktemp -d)
gh run download "$RUN" -D "$OUT"
FILES=$(find "$OUT" -type f \( -name '*.dmg' -o -name '*.exe' -o -name '*.gz' -o -name '*.deb' \))
if gh release view "$TAG" -R "$PUBLIC" >/dev/null 2>&1; then
  # shellcheck disable=SC2086
  gh release upload "$TAG" -R "$PUBLIC" --clobber $FILES
else
  # shellcheck disable=SC2086
  gh release create "$TAG" -R "$PUBLIC" --title "Show Master $V" --notes "$NOTES" $FILES
fi
echo "Published $TAG to https://github.com/$PUBLIC/releases"
