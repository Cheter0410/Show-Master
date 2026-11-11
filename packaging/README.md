# Packaging

| Ziel | Stand | Wie |
|---|---|---|
| Portable (alle Plattformen) | fertig, in der CI gebaut | `./packaging/portable.sh` → `dist/showmaster-<version>-<os>-<arch>.tar.gz/.zip` |
| Linux `.deb` | konfiguriert, CI-Job `deb` | `cargo install cargo-deb && cargo deb -p showmaster` |
| Linux AppImage | offen | `linuxdeploy` mit `packaging/linux/showmaster.desktop` |
| macOS `.dmg` | offen | App-Bundle + `codesign` + `notarytool`; braucht ein Apple-Developer-Zertifikat |
| Windows `.msi`/NSIS | offen | z. B. `cargo wix`; braucht ein Code-Signing-Zertifikat |

Das Programm ist eine einzelne ausführbare Datei mit eingebetteter Oberfläche und hat zur Laufzeit keine
weiteren Abhängigkeiten außer den System-Audio/MIDI-Bibliotheken (unter Linux `libasound2`). Alle Pakete
sind offline installierbar; es gibt keine Aktivierung, keine Telemetrie und keinen Update-Zwang.
Signierung und Notarisierung sind ohne Zertifikate nicht möglich und deshalb nicht eingerichtet.
