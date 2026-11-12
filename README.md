# Show Master

Cue-basierte Playback- und Show-Control-Software für Theater, Events, Installationen und Konzerte –
im Stil von QLab, geschrieben in Rust, für **macOS, Windows und Linux**. Läuft vollständig offline;
einziger Internetzugriff ist die abschaltbare Update-Prüfung.

> **Stand:** Alle hardwareunabhängigen Teile sind umgesetzt und automatisiert getestet (unter Linux;
> Windows-Programm per Cross-Build und Wine geprüft). Noch nicht an echter Hardware validiert sind: Audio-Interfaces (Latenz, Multichannel), System-MIDI-Ports,
> rtp-midi gegen macOS/iOS und die signierten Installer. Details: [`docs/hardware-checkliste.md`](docs/hardware-checkliste.md).

## Schnellstart

```bash
# Voraussetzungen: Rust (stable); unter Linux zusätzlich libasound2-dev.
# Die gebaute Oberfläche (ui/dist) liegt im Repository – Node ist nur zum Ändern der UI nötig.
cargo run --release -p showmaster -- --make-demo ~/showmaster-demo
```

Das Programm öffnet sein eigenes Fenster (native Rust-App, egui). **Schließen des Fensters beendet die
Show nicht:** Audio, Timecode, MIDI und Tablets laufen im Hintergrund weiter. Das Fenster kommt zurück,
wenn man Show Master erneut startet, über das Tray-Symbol (Mac: Menüleiste) oder auf dem Mac über das
Dock. Beendet wird nur mit **Quit** im Fenster (Mac: Cmd+Q) oder *Quit Show Master* im Tray-Menü.
Parallel läuft ein lokaler Webserver
nur für Fernsteuerung: Auf Handy oder iPad im selben lokalen Netz reicht im Browser
**`showmaster.local`** – das öffnet die reine Anzeige (ohne PIN, falls keine View-PIN gesetzt ist).
Oben rechts führt der Knopf **Control** zur Fernsteuerung (GO, Pause, Stop, PANIC, Pegel, Notizen und ein Timecode-Tab mit Start/Pause/Resume/Stop/Locate; Control-PIN, Standard `1234`), **View** wieder
zurück; direkt geht es auch über `showmaster.local/remote`. **Stage** zeigt eine große Ansicht für die
Inspizienz (NEXT mit Notizen, Countdown, laufende Cues, Uhren, Cue-Sheet als PDF). Ohne PIN-Tippen geht
es mit *Setup → Remote → Pair a control device*: Tablet scannt den QR-Code und bleibt angemeldet, bis es
dort widerrufen wird. Dafür meldet sich Show Master per mDNS/Bonjour unter diesem Namen
an und lauscht zusätzlich auf dem Standard-Webport 80. Geht Port 80 nicht (belegt, oder unter Linux ohne
Berechtigung – die Pakete setzen sie per `setcap`), lautet die Adresse `showmaster.local:7400`.
Die IP-Adresse (`http://<IP>`, bzw. `http://<IP>:7400/remote`) und der QR-Code in *Setup → Remote*
funktionieren immer, auch wo Multicast gesperrt ist oder ein Android-Gerät `.local` nicht kennt.
Die RTP-MIDI-Session erscheint per Bonjour in Audio-MIDI-Setup (Mac), auf dem iPad und in rtpMIDI;
gefundene Sessions lassen sich in *Setup → MIDI* direkt verbinden.

```text
showmaster [OPTIONEN] [SHOW.smshow]
  --port <N>          HTTP-Port (Standard: Port aus der Show, 7400)
  --local-only        nur auf 127.0.0.1 lauschen
  --headless          ohne Fenster laufen (nur Engine + Webserver; unter Linux ohne Bildschirm automatisch)
  --open              zusätzlich die Weboberfläche im Browser öffnen
  --make-demo <DIR>   Demo-Show mit generierten Audiodateien anlegen und öffnen
  --no-update-check   beim Start nicht nach einer neuen Version fragen (kein Internetzugriff)
  --log-dir <DIR>     Verzeichnis für Logdateien
```

## Installation

| System | Datei | Vorgehen |
|---|---|---|
| macOS (Apple Silicon und Intel) | `ShowMaster-x.y.z.dmg` | öffnen, *Show Master* in den Ordner *Programme* ziehen, starten |
| Windows | `ShowMaster-Setup-x.y.z.exe` | ausführen und durchklicken |
| Arch Linux | `showmaster-x.y.z-1-x86_64.pkg.tar.zst` | `sudo pacman -U showmaster-*.pkg.tar.zst` |
| Debian/Ubuntu | `showmaster_x.y.z-1_amd64.deb` | doppelklicken oder `sudo apt install ./showmaster_*.deb` |
| jedes Linux | `showmaster-x.y.z-linux-x86_64.tar.gz` | entpacken, `showmaster` starten |

**Linux direkt aus dem Repository** (baut und installiert in einem Schritt, auf Arch als echtes Paket;
erneut ausführen = aktualisieren, `./install.sh --uninstall` entfernt es):

```bash
git clone https://github.com/Cheter0410/Show-Master.git && cd Show-Master && ./install.sh
```

**Mac direkt aus dem Repository** (holt den neuesten Stand, installiert fehlende Werkzeuge, baut das DMG
und installiert die App nach *Programme*; erneut ausführen = aktualisieren):

```bash
git clone https://github.com/Cheter0410/Show-Master.git ~/Desktop/dev/show-master; cd ~/Desktop/dev/show-master && bash install-mac.sh
```

Die Installer liegen auf der [Download-Seite](https://github.com/Cheter0410/show-master-releases/releases/latest)
(sobald dort eine Version veröffentlicht ist). Gebaut werden sie lokal, siehe *Pakete bauen*.

- Show Master ist eine **native App mit eigenem Fenster**. Die Weboberfläche (`/remote`, `/view`) ist für
  Tablets und andere Geräte im Netz gedacht. Ein zweiter Start holt das laufende Fenster nach vorn.
- Dateien wählt man über den **Systemdialog** (Finder bzw. Explorer); Audiodateien und Shows kann man
  auch direkt ins Fenster ziehen.
- Die Installer sind **nicht signiert**. macOS blockiert den ersten Start: *Systemeinstellungen →
  Datenschutz & Sicherheit → „Dennoch öffnen“*. Windows zeigt SmartScreen: *Weitere Informationen →
  Trotzdem ausführen*.

### Updates

Wie bei Beamweave: Beim Start fragt Show Master die Download-Seite nach einer neueren Version. Gibt es
eine, erscheint oben ein grüner Knopf **Update x.y.z**; ein Klick lädt sie, prüft die SHA-256-Prüfsumme,
installiert und startet neu. Unter Linux führt der Knopf zur Download-Seite.

- Installiert wird nur auf Knopfdruck, nie im Live-Modus, nie während Cues laufen und nur mit
  gespeicherter Show.
- Die Update-Prüfung ist der **einzige** Internetzugriff des Programms. Ohne Netz meldet sie still
  „offline“; alles andere funktioniert unverändert.
- Ganz abschalten: `--no-update-check` oder Umgebungsvariable `SHOWMASTER_NO_UPDATE_CHECK=1`.

Eine neue Version veröffentlichen: siehe [`docs/release-anleitung.md`](docs/release-anleitung.md).

## Bedienkonzept

- **Cue-Liste mit Playhead.** GO feuert den Cue am Playhead („NEXT“) und rückt weiter. GO ist
  gegen Doppelauslösung entprellt. Alles, was ein GO gemeinsam startet, beginnt auf demselben Sample.
- **Cue-Typen:** Audio, Fade, MIDI (inkl. SysEx und MSC), OSC (an Pult, QLab, Resolume, Companion …), HTTP (Beamer, Steckdosen, Web-APIs), Timecode, Control (Start/Stop/Pause/Resume/
  Load-to-Time/Arm/Disarm/GoTo/Reset), Group (parallel, Playlist, Random), Memo.
- **Kopieren / Einfügen / Vorlagen:** Cues mit Strg/Cmd+C kopieren und in derselben oder einer anderen
  Show (auch in einem zweiten Show-Master-Fenster) mit Strg/Cmd+V einfügen; Audiopfade und Fade-/Control-
  Ziele werden mitgenommen. Rechtsklick → *Save as template* speichert Cues als Vorlage, *Insert template*
  fügt sie in jede Show ein.
- **Backup-Rechner:** *Setup → Remote → Backup computer*: ein zweiter Rechner mit derselben Show läuft
  parallel mit (jedes GO/Stop/PANIC und jede Änderung wird gespiegelt). Fällt der Hauptrechner aus, wird
  einfach der Ton umgeschaltet – der Backup spielt bereits.
- **Feste Installationen:** *Setup → General → This computer*: mit dem Rechner starten, letzte Show öffnen,
  **Kiosk-Modus** (startet im Live-Modus; Bearbeiten, Setup und Beenden sind gesperrt, *Unlock* mit
  Kiosk-PIN).
- **Show-Paket:** *File → Export show package* packt Show und alle Audiodateien in eine `.smpack`-Datei;
  *Open show package* (oder Datei aufs Fenster ziehen) packt sie auf einem anderen Rechner in einen Ordner aus.
- **Cue-Sheet:** *File → Export cue sheet* als PDF (zum Ausdrucken) oder CSV (Tabelle), mit Notizen.
- **Edit-Modus:** Cues anlegen, verschieben, gruppieren, Inspector, Wellenform, Routing-Matrix,
  unbegrenztes Undo/Redo, Vorschau einzelner Cues ohne den Playhead zu bewegen.
- **Live-Modus:** schreibgeschützt (serverseitig durchgesetzt), reduziertes Dashboard mit großen
  Bedienelementen, Uhren, Pegeln und Event-Log. Beim Wechsel läuft ein Pre-Flight-Check.
- **Standard-Hotkeys:** `Leertaste` GO · `Esc` Panic · `↑/↓` Playhead · `P` Pause/Resume · `S` Stop All ·
  `V` Vorschau · `L` Edit/Live · `Strg+Z` Undo. Frei belegbar mit Konfliktprüfung.
- **Timecode:** LTC (als Audiosignal), MTC und rtp-midi gleichzeitig; Cues können an Timecode-Zeiten
  hängen. Verhalten bei Sprüngen je Cue-Liste: *fire*, *skip* oder *chase*.
- **Fernsteuerung:** iPad per Browser (PWA-fähig), Stream Deck / Bitfocus Companion über die lokale
  HTTP-API oder OSC – siehe [`docs/remote-api.md`](docs/remote-api.md) und [`docs/streamdeck.md`](docs/streamdeck.md).

## Architektur in Kürze

```
 Hotkeys ─┐                               ┌─► Audio-Thread (Mixer, LTC)  ── Interface
 Web-UI ──┤   Command-Bus    ┌──────────┐ │
 iPad ────┼──► (1 Thread) ──►│Cue-Engine│─┼─► MIDI-Sender (System-Ports, rtp-midi, MTC)
 HTTP/OSC ┘        ▲         └──────────┘ │
                   └── State-Diff / Tick / Log ◄── Events
```

| Crate | Aufgabe |
|---|---|
| `sm-timecode` | Framerates, sample-verankerter Transport, LTC-Encoder + unabhängiger Decoder, MTC |
| `sm-rtpmidi` | eigene AppleMIDI/RTP-MIDI-Implementierung (UDP, ohne Treiber), Recovery-Journal senden + empfangen |
| `sm-mdns` | optionales mDNS/Bonjour: Weboberfläche + RTP-MIDI ankündigen, RTP-MIDI-Sessions finden (nur lokales Multicast) |
| `sm-audio` | Echtzeit-Mixer (Routing-Matrix, Fades, Streaming), Backends hinter Traits (cpal, Null) |
| `sm-core` | Show-Modell, Showfile (Migration, Atomic Write, Backups), deterministische Cue-Engine, Pre-Flight |
| `sm-io` | Engine-Aktionen → Audio, MIDI, LTC, MTC |
| `sm-server` | Command-Bus, Edit/Undo, HTTP + WebSocket (Rollen, State-Diff), OSC, Updater |
| `sm-desktop` | natives Fenster (egui): Edit-Modus, Live-Dashboard, Setup, Systemdialoge |
| `showmaster` | das Programm (Logging, Panic-Hook, CLI) |
| `ui/` | Web-Oberfläche für Tablets (React + TypeScript), ins Binary eingebettet |

**Eine Master-Clock:** die Sample-Clock des Audio-Streams. Cue-Scheduling, Fades, LTC, MTC und MIDI
leiten sich davon ab; die Engine liest nie eine Wanduhr und ist deshalb mit simulierter Zeit testbar.
Ausführlich: [`docs/architecture.md`](docs/architecture.md), Entscheidungen: [`docs/decisions.md`](docs/decisions.md).

## Entwickeln und Testen

```bash
cargo test --workspace                 # alle Tests (ohne Hardware)
cargo clippy --workspace --all-targets -- -D warnings
./scripts/offline-test.sh              # Nachweis: läuft ohne Netz, versucht keinen Zugriff nach außen
./scripts/bench-startup.sh             # Startzeit und Speicherbedarf
SM_SOAK_SECONDS=7200 cargo test --release -p sm-server --test soak -- --ignored --nocapture   # Dauertest

cargo run -p sm-timecode --example ltc_roundtrip        # Prototyp (b): LTC mit Decoder-Gegenprobe
cargo run -p sm-rtpmidi --example rtp_midi_handshake    # Prototyp (c): rtp-midi-Handshake
cargo run --release -p sm-audio --example audio_bench   # Prototyp (a): Multichannel-Streaming

cargo build -p showmaster && node ui/e2e/real.mjs        # Browser-Test der echten App (braucht Playwright + Chromium)
cd ui && npm run dev                   # UI-Entwicklung gegen einen laufenden showmaster
cd ui && npm run build                 # nach UI-Änderungen: ui/dist neu bauen und mit committen
```

Offline-Builds: `Cargo.lock` und `ui/package-lock.json` sind eingecheckt; mit `cargo vendor` und einem
npm-Cache lässt sich ohne Internet bauen. Zur Laufzeit wird nie eine Internetverbindung benötigt.

Optionale Cargo-Features: `--features sm-io/asio` (Windows, ASIO-SDK nötig), `--features sm-io/jack`.

## Pakete bauen

Lokal, ohne CI:

- **Linux-PC** (Arch oder Debian/Ubuntu): `./packaging/build-on-linux.sh` baut Linux-Archiv, Arch-Paket
  (`.deb` falls `cargo-deb` installiert ist), Windows-Programm und Windows-Installer (Inno Setup unter Wine).
- **Mac**: `bash packaging/macos/setup-and-build.sh` baut das DMG (Universal: Intel + Apple Silicon).

Einrichtung und Veröffentlichung: [`docs/release-anleitung.md`](docs/release-anleitung.md),
Details: [`packaging/README.md`](packaging/README.md).

## Lizenz

MIT oder Apache-2.0, nach Wahl.
