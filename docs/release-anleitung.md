# Releases bauen und veröffentlichen

Ohne CI: Linux und Windows auf dem **Arch-Linux-PC**, das DMG auf dem **Mac**. Veröffentlicht wird mit
der GitHub-Kommandozeile `gh` nach `Cheter0410/show-master-releases`, wo der eingebaute Updater nachsieht.

## Einmalig: Arch-Linux-PC einrichten

```bash
sudo pacman -S --needed base-devel git github-cli rustup alsa-lib gtk3 libxkbcommon-x11 mingw-w64-gcc wine
rustup default stable
rustup target add x86_64-pc-windows-gnu
gh auth login
git clone -b claude/nifty-heisenberg-mcqrc9 https://github.com/Cheter0410/Show-Master.git ~/Show-Master
```

`wine` liegt im Repository *multilib*; falls pacman es nicht findet, in `/etc/pacman.conf` den Abschnitt
`[multilib]` einkommentieren und `sudo pacman -Sy` ausführen.

Inno Setup (baut den Windows-Installer) einmalig unter Wine installieren:

```bash
curl -L -o /tmp/innosetup.exe https://github.com/jrsoftware/issrc/releases/download/is-6_7_4/innosetup-6.7.4.exe
WINEDEBUG=-all WINEDLLOVERRIDES="mscoree,mshtml=" wine /tmp/innosetup.exe /VERYSILENT /SUPPRESSMSGBOXES /NORESTART /CURRENTUSER '/DIR=C:\InnoSetup'
ls ~/.wine/drive_c/InnoSetup/ISCC.exe
```

Optional für Debian/Ubuntu-Pakete: `cargo install cargo-deb --locked`.

Optional für das AppImage: `appimagetool` aus dem AUR (`appimagetool-bin`, z. B. `yay -S appimagetool-bin`)
– oder gar nichts installieren und beim Bauen `APPIMAGE=1` setzen, dann lädt das Skript `appimagetool` einmal
nach `~/.cache/showmaster-build/`.

Optional zum Signieren der Windows-Dateien: `sudo pacman -S osslsigncode` und ein Code-Signing-Zertifikat als
`.pfx` (siehe unten).

## Einmalig: Mac einrichten

Nichts außer Git – das Skript installiert die Xcode-Werkzeuge und Rust selbst, wenn sie fehlen. Für das
Hochladen zusätzlich die GitHub-Kommandozeile (`brew install gh`, dann `gh auth login`) oder den Upload
über die Website.

## Eine Version bauen

Versionsnummer: `Cargo.toml` (`[workspace.package] version`) und ein Abschnitt `## x.y.z` oben in
`CHANGELOG.md`. Nach dem Ändern `cargo update -w`, committen, pushen.

**Arch-PC** (Linux + Windows):

```bash
cd ~/Show-Master && git pull && ./packaging/build-on-linux.sh
```

Mit AppImage (lädt `appimagetool` beim ersten Mal herunter, falls es nicht installiert ist):

```bash
APPIMAGE=1 ./packaging/build-on-linux.sh
```

**Mac** (DMG):

```bash
cd ~/Desktop/dev/show-master && git pull && bash packaging/macos/setup-and-build.sh
```

## Signieren (optional)

Ohne diese Variablen bleibt alles wie bisher (Mac ad hoc signiert, Windows unsigniert). Details:
[`packaging/README.md`](../packaging/README.md#code-signing-optional).

**Windows** (Arch-PC): Authenticode-Zertifikat (OV, als `.pfx`) bei einer Zertifizierungsstelle wie Certum,
Sectigo oder SSL.com kaufen; `osslsigncode` installieren. Dann:

```bash
WIN_SIGN_PFX=~/keys/showmaster.pfx WIN_SIGN_PASSWORD='…' ./packaging/build-on-linux.sh
# optional: WIN_SIGN_TIMESTAMP_URL=http://timestamp.sectigo.com
```

**Mac**: Im Apple Developer Program (99 USD/Jahr) ein Zertifikat *Developer ID Application* erzeugen (Xcode →
Settings → Accounts → Manage Certificates → +). Einmalig die Zugangsdaten für die Notarisierung speichern
(app-spezifisches Passwort von appleid.apple.com):

```bash
xcrun notarytool store-credentials showmaster --apple-id du@example.com --team-id TEAMID --password xxxx-xxxx-xxxx-xxxx
security find-identity -v -p codesigning   # zeigt den genauen Namen der Identität
SIGN_IDENTITY="Developer ID Application: Name (TEAMID)" NOTARY_PROFILE=showmaster \
  bash packaging/macos/setup-and-build.sh
```

Zeitstempel und Notarisierung brauchen Internet auf dem Build-Rechner, nie auf den Show-Rechnern.

## Veröffentlichen

Wer zuerst fertig ist, legt das Release an (`create`), der andere lädt dazu (`upload`). Auf dem Arch-PC:

```bash
cd ~/Show-Master
V=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)
awk -v v="$V" '$0 ~ "^## " v "$" {on=1; next} /^## / {on=0} on' CHANGELOG.md > /tmp/notes.md
gh release view "v$V" -R Cheter0410/show-master-releases >/dev/null 2>&1 \
  && gh release upload "v$V" -R Cheter0410/show-master-releases --clobber dist/* \
  || gh release create "v$V" -R Cheter0410/show-master-releases --title "Show Master $V" --notes-file /tmp/notes.md dist/*
```

Auf dem Mac genauso (dort enthält `dist/` das DMG und die zwei macOS-`.gz`). Ohne `gh`: auf
github.com/Cheter0410/show-master-releases → *Releases* → das Release öffnen bzw. *Draft a new release*
mit Tag `vX.Y.Z` → Dateien hineinziehen.

Prüfen: `gh release view "v$V" -R Cheter0410/show-master-releases` – vollständig sind: `.dmg`,
`ShowMaster-Setup-….exe`, `.pkg.tar.zst`, `linux-x86_64.tar.gz` und die drei `showmaster-…gz` (dazu, falls
gebaut, `.AppImage` und `.deb`).

## Updater einmal komplett testen

1. Version `0.1.0` wie oben veröffentlichen und installieren (Mac und/oder Windows).
2. In `Cargo.toml` auf `0.1.1` anheben, `## 0.1.1` mit einer Zeile in `CHANGELOG.md`, `cargo update -w`,
   committen, pushen; bauen und veröffentlichen wie oben.
3. Die installierte 0.1.0 starten: oben erscheint **Update 0.1.1** → *Update and restart*. Danach muss die
   App von selbst als 0.1.1 neu starten.
