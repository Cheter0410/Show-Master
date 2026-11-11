# Packaging

| Ziel | Datei | Wie |
|---|---|---|
| macOS | `ShowMaster-x.y.z.dmg` (Universal: Apple Silicon + Intel) | Workflow `build`, Job `macos` |
| Windows | `ShowMaster-Setup-x.y.z.exe` (Inno Setup, `packaging/windows/showmaster.iss`) | Workflow `build`, Job `windows` |
| Debian/Ubuntu | `showmaster_x.y.z-1_amd64.deb` | Workflow `build`, Job `linux` (`cargo deb -p showmaster`) |
| Portable | `showmaster-<version>-<os>-<arch>.tar.gz/.zip` | `./packaging/portable.sh`, auch im CI-Workflow |
| Updater-Patches | `showmaster-macos-arm64.gz`, `showmaster-macos-x86_64.gz`, `showmaster-windows-x86_64.gz` | Workflow `build` |

Jeder Job testet sein Paket nach dem Bau: installieren bzw. aus dem DMG kopieren, starten, API abfragen,
über die API beenden.

## Veröffentlichen

Der Updater liest `https://github.com/Cheter0410/show-master-releases` (öffentliches Repository nur für
Downloads, wie `beamweave-releases`). Zwei Wege:

1. **Automatisch:** In diesem Repository das Secret `RELEASES_TOKEN` anlegen (Token mit Schreibrecht auf
   die Releases von `show-master-releases`). Ein Tag `vX.Y.Z` baut dann alles und veröffentlicht es.
2. **Von Hand:** `scripts/release.sh` (braucht die `gh`-CLI): taggt, wartet auf den Build und lädt hoch.

Die Release-Notizen kommen aus dem Abschnitt `## X.Y.Z` in `CHANGELOG.md`.

## Nicht signiert

Für signierte und notarisierte macOS-Apps braucht es ein Apple-Developer-Zertifikat, für signierte
Windows-Installer ein Code-Signing-Zertifikat. Beides fehlt; die App ist ad hoc signiert. Folge: macOS
verlangt beim ersten Start „Dennoch öffnen“, Windows zeigt SmartScreen.

## Offen

- AppImage für andere Linux-Distributionen.
- Der Updater installiert unter Linux nicht selbst, sondern verweist auf die Download-Seite.
