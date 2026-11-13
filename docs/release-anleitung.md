# Releases veröffentlichen

Ein Befehl, auf beiden Rechnern derselbe:

```bash
./release.sh
```

- **Arch-PC** baut Linux und Windows, **Mac** baut das DMG. Jeder lädt seinen Teil hoch.
- **Jeder Rechner veröffentlicht für sich – keiner wartet auf den anderen.** Wer gerade Zeit hat, lässt
  `./release.sh` laufen; die Version ist danach sofort für seine Systeme da. Die anderen Systeme bleiben
  so lange auf der letzten Version, die es für sie gibt (das Programm und Stagehand nehmen immer das
  neueste Release, das eine Datei für das eigene System hat).
- **Die Versionsnummer ist automatisch:**
  - Fehlen im neuesten Release die Teile dieses Rechners, baut er genau dessen Stand und reicht sie nach.
  - Sind sie da und es gibt neue Änderungen, wird die Version um eins erhöht (0.4.0 → 0.4.1). „Was ist
    neu“ kommt aus den Commit-Nachrichten seitdem (in `CHANGELOG.md` und im Update-Dialog), dann wird
    gepusht, gebaut und veröffentlicht.
  - Hat sich seit der letzten Version nichts geändert, passiert nichts.
- Wer mehrere Versionen verpasst hat, baut nur die neueste nach – die dazwischen werden übersprungen.
- Beide Rechner bauen garantiert **denselben Code**: jede Version bekommt im Code-Repo einen Git-Tag
  (`v0.4.1`), und gebaut wird immer dieser Stand, auch wenn danach schon weiter gearbeitet wurde.
- Beim ersten Mal richtet das Skript alles Fehlende selbst ein: GitHub-Anmeldung (Browser öffnet sich),
  Rust, auf dem Arch-PC die Werkzeuge für den Windows-Installer (MinGW, Wine, Inno Setup).

Was schon oben ist:

```bash
./release.sh status
```

```
Show Master 0.4.1 · veröffentlicht
  ○ Mac      noch nicht da – reicht der Mac nach
  ✓ Windows  ShowMaster-Setup-0.4.1.exe
  ✓ Linux    showmaster-0.4.1-linux-x86_64.tar.gz
```

Auf dem Bildschirm steht jeder Schritt mit Spinner und Laufzeit, danach ✓, ↷ (übersprungen, weil schon
erledigt) oder ✗. Alles andere (Compiler, Tests, Installer) steht im Protokoll, dessen Pfad am Ende
angezeigt wird; bei einem Fehler zeigt das Skript nur den betroffenen Teil daraus.

Nichts wird doppelt gemacht: die Tests laufen einmal pro Stand, Pakete und Installer werden nur neu
gebaut, wenn sich das Programm darin geändert hat, und hochgeladen wird nur, was noch fehlt oder sich
geändert hat. Ein zweiter Lauf nach einem Abbruch macht also genau da weiter. Die Tests laufen ohne echte
Audiogeräte (nur die stumme Ausgabe), damit sie den Ton des Rechners nicht stören.

Tipp: Commit-Nachrichten so schreiben, dass sie als „Was ist neu“ taugen (z. B. „Waveform zoom“).

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
