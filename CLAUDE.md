# Show Master – notes for Claude Code

Cue-based playback and show control (audio, MIDI, OSC, timecode, tablet remote) for macOS, Windows
and Linux. Rust workspace (`crates/*`) plus a web UI in `ui/` (Vite + TypeScript).

## Who works here

Two people work on this repository, each with Claude Code: **Cheter0410** (owner) and **jay212-1**
(Jan). Both push to the same branch, so the rules below exist to keep us on the same state and away
from merge conflicts. Follow them in every session.

## Git workflow

- **Work directly on `main`.** No feature branches, no pull requests (agreed on 2026-11-15).
- **Before you start:** `git pull --rebase`. The other person (or a release run) may have pushed.
- **Before every push:** `git pull --rebase` again, then build and test, then `git push`.
- **Commit and push soon** – small commits, pushed the same day. Unpushed work piling up is what makes
  conflicts.
- Never force-push `main`, never rewrite pushed history, never delete or move tags (`vX.Y.Z` tags are
  what the release builds).
- A rebase stops on a conflict: resolve it, `git rebase --continue`. When unsure whose change wins,
  stop and ask instead of guessing.

## Version and changelog with every change

- Every change raises the version: `version = "x.y.z"` in `Cargo.toml` (`[workspace.package]`),
  then `cargo update -w` so `Cargo.lock` follows. Small changes: patch step (z+1); bigger features: minor step.
- Add a `## x.y.z` section at the top of `CHANGELOG.md` (English, like the entries below it).
- Do the bump **right after** `git pull --rebase`, just before committing – never on a stale state.
- **Conflict on the version** (both of us bumped): keep the other one's version and entry, put yours
  on top as the next number. In `CHANGELOG.md` keep both sections, newest first.
- **Conflict in `Cargo.lock`:** never merge it by hand. Take the file that is already on `main`
  (during a rebase that is `git checkout --ours Cargo.lock`), run `cargo update -w` and `cargo build`,
  then `git add Cargo.lock` and continue.
- `release.sh` also bumps the version by itself when the released version has changes after it, and
  commits, tags and pushes that. So pull before you work – a release may have added a commit.

## Never commit

Build output and local files: `target/`, `dist/`, `.DS_Store`, logs, editor files. They are in
`.gitignore`; keep it that way.

## Releases

Released files go to the public repository **`Cheter0410/show-master-releases`**. It only holds the downloads (and a
README) – never develop there. Installed copies and Showtime Suite read the newest release from it.

`./release.sh` publishes (Arch PC: Linux + Windows, Mac: the DMG); `./release.sh status` shows what is
up. Each computer publishes on its own. Details: `docs/release-anleitung.md`.

## The other projects

All live side by side in `~/Desktop/dev` (folder names in lower case):

| Folder | Repository | Release downloads |
|---|---|---|
| `show-master` | Cheter0410/Show-Master | Cheter0410/show-master-releases |
| `lantern` | Cheter0410/lantern | Cheter0410/lantern-releases |
| `showtime-suite` | Cheter0410/showtime-suite | Cheter0410/showtime-suite-releases |
| `beamweave` | Cheter0410/beamweave | Cheter0410/beamweave-releases |

Showtime Suite installs, updates and starts the others; its catalog is `showtime-suite/apps.json`.

## Build and test

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cd ui && npm run build        # after UI changes: rebuild ui/dist and commit it too
```

More (offline test, soak test, prototypes, browser end-to-end test): README, "Entwickeln und Testen".
Architecture and past decisions: `docs/architecture.md`, `docs/decisions.md` – read them before
bigger changes and add a line to `decisions.md` when you decide something new.

## Project notes

- `ui/dist` is committed (the program serves it). Never merge it by hand: on a conflict, take
  either side, run `npm run build` and commit the result.
- The crates `sm-timecode`, `sm-rtpmidi` and `sm-mdns` also exist as copies in `lantern/crates`. A fix
  in one usually belongs in the other too – say so in the commit message.
- The GitHub default branch was still `claude/nifty-heisenberg-mcqrc9` on 2026-11-15; `main` is the
  branch to use. Cheter0410 (owner): please switch the default branch to `main` in the repository
  settings and delete the old branch.
- The checkout folder is `show-master` (lower case), as Showtime Suite expects.
- `scripts/setup.sh` is the same file in Lantern, Showtime Suite and here – change all three together.
