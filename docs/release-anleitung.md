# Releases veröffentlichen

Ein Befehl, auf beiden Rechnern derselbe:

```bash
./release.sh
```

- **Arch-PC** baut Linux und Windows, **Mac** baut das DMG. Jeder lädt seinen Teil hoch.
- Hochgeladen wird in einen **Entwurf** (für Nutzer unsichtbar). Sobald Mac, Windows und Linux drin
  sind, veröffentlicht das Skript das Release automatisch – egal wer als Letzter fertig ist. Erst dann
  bieten installierte Show Master (und Stagehand) das Update an.
- Beim ersten Mal richtet das Skript alles Fehlende selbst ein: GitHub-Anmeldung (Browser öffnet sich),
  Rust, auf dem Arch-PC die Werkzeuge für den Windows-Installer (MinGW, Wine, Inno Setup).

## Ablauf einer neuen Version

1. **Einer** erhöht die Version (schreibt auch, was neu ist):

   ```bash
   ./release.sh version
   ```

   Es öffnet sich `CHANGELOG.md`: unter der neuen Version kurz eintragen, was neu ist (das steht später
   im Update-Dialog), speichern, schließen. Das Skript committet und pusht.
2. **Beide** auf ihrem Rechner: `./release.sh` (holt vorher selbst den neuesten Stand).
3. Nachsehen, was schon oben ist:

   ```bash
   ./release.sh status
   ```

   ```
   Show Master 0.4.1: Entwurf (noch nicht für Nutzer sichtbar)
     · Mac      fehlt noch - baut der Mac mit ./release.sh
     ✓ Windows  ShowMaster-Setup-0.4.1.exe
     ✓ Linux    showmaster-0.4.1-linux-x86_64.tar.gz
   ```

Eine schon veröffentlichte Version wird nie überschrieben – dann sagt das Skript, dass zuerst
`./release.sh version` nötig ist.

## Einmalig: Zugang für den zweiten Rechner

Der Besitzer der Release-Repos lädt den Freund ein: auf GitHub
`Cheter0410/show-master-releases` → **Settings → Collaborators → Add people** (dasselbe für
`Cheter0410/stagehand-releases` und, damit er pushen kann, für die Code-Repos). Der Freund nimmt die
Einladung an (E-Mail oder https://github.com/notifications). Fehlt das Recht, sagt `./release.sh` genau
das und zeigt den Link zur Einladung.

## Wo liegen die Repos?

| | Mac | Arch-PC |
|---|---|---|
| Show Master | `~/Desktop/dev/show-master` | wo `install.sh` geklont wurde |
| Stagehand | `~/Desktop/dev/stagehand` | wo `install.sh` geklont wurde |

In jedem Repo heißt der Befehl gleich: `./release.sh` (auf dem Mac auch `bash release.sh`).

## Anhang: Signieren (optional)

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
