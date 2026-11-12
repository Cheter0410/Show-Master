# Packaging

Gebaut wird lokal: Linux und Windows auf einem Linux-PC (Arch oder Debian/Ubuntu), macOS auf einem Mac.
Es gibt keine CI.

| Ziel | Datei | Wo / wie |
|---|---|---|
| macOS (Universal: Intel + Apple Silicon) | `ShowMaster-x.y.z.dmg` | Mac: `bash packaging/macos/setup-and-build.sh` |
| Windows | `ShowMaster-Setup-x.y.z.exe` (Inno Setup, `packaging/windows/showmaster.iss`) | Linux-PC: `./packaging/build-on-linux.sh` (Cross-Build mit MinGW, Inno Setup unter Wine) |
| Arch Linux | `showmaster-x.y.z-1-x86_64.pkg.tar.zst` (`packaging/arch/PKGBUILD`) | Linux-PC mit `makepkg` |
| Debian/Ubuntu | `showmaster_x.y.z-1_amd64.deb` | Linux-PC mit `cargo-deb` |
| jedes Linux | `showmaster-x.y.z-linux-x86_64.tar.gz` | Linux-PC |
| jedes Linux (eine Datei) | `ShowMaster-x.y.z-x86_64.AppImage` | Linux-PC mit `appimagetool` oder `APPIMAGE=1` (lädt es herunter) |
| Updater-Patches | `showmaster-macos-arm64.gz`, `showmaster-macos-x86_64.gz`, `showmaster-windows-x86_64.gz` | entstehen mit DMG bzw. Windows-Build |

Alles landet in `dist/`. Die gebaute Weboberfläche (`ui/dist`) liegt im Repository; Node.js wird zum
Bauen nicht gebraucht.

## Veröffentlichen

Der Updater liest `https://github.com/Cheter0410/show-master-releases`. Ablauf und alle Befehle:
[`docs/release-anleitung.md`](../docs/release-anleitung.md).

## AppImage

`packaging/build-on-linux.sh` baut `dist/ShowMaster-x.y.z-x86_64.AppImage`, wenn `appimagetool` installiert
ist (Arch/AUR: `appimagetool-bin`) oder mit `APPIMAGE=1` (lädt `appimagetool` einmal nach
`~/.cache/showmaster-build/`; andere Quelle mit `APPIMAGETOOL_URL=...`). `SKIP_APPIMAGE=1` überspringt den
Schritt. Inhalt: `AppRun` (`packaging/linux/AppRun`), `usr/bin/showmaster`, `.desktop`-Datei und Icon.

Laufzeit-Abhängigkeiten kommen vom System, nicht aus dem AppImage: ALSA (`alsa-lib`), GTK 3 (Dateidialoge),
`libxkbcommon-x11`, die Grafiktreiber (Vulkan bzw. OpenGL/Mesa). Auf Systemen ohne FUSE 2 startet das
AppImage mit `--appimage-extract-and-run` bzw. `APPIMAGE_EXTRACT_AND_RUN=1`. Der Updater ersetzt unter Linux
nichts selbst, also auch nicht das AppImage: neue Datei herunterladen.

## Code-Signing (optional)

Ohne die folgenden Variablen laufen die Skripte wie bisher: Mac-App ad hoc signiert (macOS verlangt beim
ersten Start „Dennoch öffnen“), Windows unsigniert (SmartScreen-Warnung).

**macOS** (`packaging/macos/build-dmg.sh`): Zertifikat *Developer ID Application* aus dem Apple Developer
Program (99 USD/Jahr; in Xcode → Settings → Accounts → Manage Certificates erzeugen, landet im
Schlüsselbund).

| Variable | Wirkung |
|---|---|
| `SIGN_IDENTITY` | z. B. `Developer ID Application: Name (TEAMID)`: signiert mit Hardened Runtime und Zeitstempel, Entitlements aus `packaging/macos/entitlements.plist` (nur Audio-Eingang für LTC; die App ist nicht gesandboxt, Netzwerk braucht daher nichts), signiert auch das DMG |
| `NOTARY_PROFILE` | Name eines mit `xcrun notarytool store-credentials <name> --apple-id … --team-id … --password <app-spezifisches Passwort>` gespeicherten Profils: notarisiert und „staplet“ App und DMG (die App einzeln, damit Gatekeeper auch auf Show-Rechnern ohne Internet prüfen kann) |

Hinweis: Ein Programm-Update über den eingebauten Updater signiert die App danach wieder ad hoc (sie läuft
weiter, ist aber nicht mehr Developer-ID-signiert); volle Updates per DMG behalten die Signatur.

**Windows** (`packaging/build-on-linux.sh`, Signieren unter Linux mit `osslsigncode`, Arch:
`sudo pacman -S osslsigncode`): ein Authenticode-Code-Signing-Zertifikat als `.pfx` (z. B. von Certum,
Sectigo, SSL.com; OV reicht, EV-Zertifikate liegen meist auf einem Hardware-Token und gehen so nicht).

| Variable | Wirkung |
|---|---|
| `WIN_SIGN_PFX` | Pfad zur `.pfx`-Datei: signiert `showmaster.exe` (vor dem Packen, also auch im Updater-`.gz`) und `ShowMaster-Setup-x.y.z.exe` |
| `WIN_SIGN_PASSWORD` | Passwort der `.pfx` (wird über eine temporäre Datei mit Rechten 600 übergeben, nicht auf der Kommandozeile) |
| `WIN_SIGN_TIMESTAMP_URL` | RFC-3161-Zeitstempeldienst, Standard `http://timestamp.digicert.com` |

Zeitstempel brauchen Internet auf dem Build-PC (nicht auf den Show-Rechnern). Ist `WIN_SIGN_PFX` gesetzt,
aber `osslsigncode` fehlt, bricht das Skript ab statt unsigniert zu bauen.

## Linux-Updates

Der Updater installiert unter Linux nicht selbst, sondern verweist auf die Download-Seite (Arch: neues Paket
mit `pacman -U` installieren).
