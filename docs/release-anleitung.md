# Releases bauen und veröffentlichen

Ohne GitHub Actions: Mac-Installer auf dem Mac, Linux-Paket und Windows-Installer auf dem Linux-PC,
veröffentlicht mit der GitHub-Kommandozeile `gh` nach `Cheter0410/show-master-releases`.

## Einmalig: Mac einrichten

```bash
xcode-select --install
brew install node gh
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
gh auth login
```

## Einmalig: Linux-PC einrichten (Ubuntu/Debian)

```bash
sudo apt update
sudo apt install -y build-essential pkg-config libasound2-dev curl git gh gcc-mingw-w64-x86-64 wine
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source "$HOME/.cargo/env"
rustup target add x86_64-pc-windows-gnu
cargo install cargo-deb --locked
curl -fsSL https://deb.nodesource.com/setup_22.x | sudo -E bash -
sudo apt install -y nodejs
gh auth login
```

Inno Setup (baut den Windows-Installer) unter Wine installieren:

```bash
curl -L -o /tmp/innosetup.exe https://github.com/jrsoftware/issrc/releases/download/is-6_7_4/innosetup-6.7.4.exe
WINEDEBUG=-all WINEDLLOVERRIDES="mscoree,mshtml=" wine /tmp/innosetup.exe /VERYSILENT /SUPPRESSMSGBOXES /NORESTART /CURRENTUSER '/DIR=C:\InnoSetup'
ls ~/.wine/drive_c/InnoSetup/ISCC.exe
```

(Heißt der Befehl auf deinem System `wine64` statt `wine`, dort entsprechend ersetzen.)

## Eine Version veröffentlichen

Versionsnummer: steht in `Cargo.toml` (`[workspace.package] version`). Für jede neue Version dort und
in `CHANGELOG.md` (neuer Abschnitt `## x.y.z`) anheben, committen und pushen.

### 1. Auf dem Mac

```bash
cd Show-Master && git pull
./packaging/macos/build-dmg.sh
V=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)
awk -v v="$V" '$0 ~ "^## " v "$" {on=1; next} /^## / {on=0} on' CHANGELOG.md > /tmp/notes.md
gh release create "v$V" -R Cheter0410/show-master-releases --title "Show Master $V" --notes-file /tmp/notes.md \
  "dist/ShowMaster-$V.dmg" dist/showmaster-macos-arm64.gz dist/showmaster-macos-x86_64.gz
```

### 2. Auf dem Linux-PC

```bash
cd Show-Master && git pull
./packaging/build-on-linux.sh
V=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)
gh release upload "v$V" -R Cheter0410/show-master-releases --clobber \
  dist/showmaster_"$V"*.deb "dist/ShowMaster-Setup-$V.exe" dist/showmaster-windows-x86_64.gz
```

### 3. Prüfen

```bash
gh release view "v$V" -R Cheter0410/show-master-releases
```

Es müssen sechs Dateien dabei sein: `.dmg`, `Setup-….exe`, `.deb` und die drei `showmaster-…gz`.

## Updater testen (einmal komplett)

1. Version `0.1.0` wie oben veröffentlichen und auf dem Mac (bzw. Windows) installieren.
2. Version auf `0.1.1` anheben: in `Cargo.toml` `version = "0.1.0"` → `"0.1.1"`, in `CHANGELOG.md`
   ganz oben einen Abschnitt `## 0.1.1` mit einer Zeile ergänzen. Dann:
   ```bash
   cargo update -w
   git commit -am "Release 0.1.1" && git push
   ```
3. Schritte 1–2 von „Eine Version veröffentlichen“ für `0.1.1` ausführen.
4. Installierte 0.1.0 starten: Oben erscheint **Update 0.1.1**. Klicken → *Update and restart*.
   Danach muss die App von selbst neu starten und `0.1.1` anzeigen (`showmaster --version`).

## Optional: Releases wieder automatisch über GitHub Actions

Sobald Actions wieder läuft: Secret `RELEASES_TOKEN` (Token mit Schreibrecht auf
`show-master-releases`) im Repository `Show-Master` anlegen. Dann reicht:

```bash
git tag v0.1.0 && git push origin v0.1.0
```
