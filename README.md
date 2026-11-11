# Show Master

Cue-basierte Playback- und Show-Control-Software für Theater, Events, Installationen und Konzerte –
im Stil von QLab, geschrieben in Rust, für **macOS, Windows und Linux**. Läuft vollständig offline.

> **Stand:** Alle hardwareunabhängigen Teile sind umgesetzt und automatisiert getestet. Noch nicht
> an echter Hardware validiert sind: Audio-Interfaces (Latenz, Multichannel), System-MIDI-Ports,
> rtp-midi gegen macOS/iOS und die signierten Installer. Details: [`docs/hardware-checkliste.md`](docs/hardware-checkliste.md).

## Schnellstart

```bash
# Voraussetzungen: Rust (stable), Node 22; unter Linux zusätzlich libasound2-dev
cd ui && npm ci && npm run build && cd ..
cargo run --release -p showmaster -- --make-demo ~/showmaster-demo --open
```

Das Programm startet die Engine und einen lokalen Webserver und öffnet die Oberfläche im Browser
(`http://127.0.0.1:7400`). Tablets im selben lokalen Netz erreichen die Fernsteuerung unter
`http://<IP>:7400/remote` (PIN, Standard `1234`) und die reine Anzeige unter `/view`; URL und QR-Code
stehen in *Setup → Remote*.

```text
showmaster [OPTIONEN] [SHOW.smshow]
  --port <N>          HTTP-Port (Standard: Port aus der Show, 7400)
  --local-only        nur auf 127.0.0.1 lauschen
  --open              Oberfläche im Standardbrowser öffnen
  --make-demo <DIR>   Demo-Show mit generierten Audiodateien anlegen und öffnen
  --log-dir <DIR>     Verzeichnis für Logdateien
```

## Bedienkonzept

- **Cue-Liste mit Playhead.** GO feuert den Cue am Playhead („NEXT“) und rückt weiter. GO ist
  gegen Doppelauslösung entprellt. Alles, was ein GO gemeinsam startet, beginnt auf demselben Sample.
- **Cue-Typen:** Audio, Fade, MIDI (inkl. SysEx und MSC), Timecode, Control (Start/Stop/Pause/Resume/
  Load-to-Time/Arm/Disarm/GoTo/Reset), Group (parallel, Playlist, Random), Memo.
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
| `sm-rtpmidi` | eigene AppleMIDI/RTP-MIDI-Implementierung (UDP, ohne Treiber) |
| `sm-audio` | Echtzeit-Mixer (Routing-Matrix, Fades, Streaming), Backends hinter Traits (cpal, Null) |
| `sm-core` | Show-Modell, Showfile (Migration, Atomic Write, Backups), deterministische Cue-Engine, Pre-Flight |
| `sm-io` | Engine-Aktionen → Audio, MIDI, LTC, MTC |
| `sm-server` | Command-Bus, Edit/Undo, HTTP + WebSocket (Rollen, State-Diff), OSC |
| `showmaster` | das Programm (Logging, Panic-Hook, CLI) |
| `ui/` | Web-Oberfläche (React + TypeScript), ins Binary eingebettet |

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
```

Offline-Builds: `Cargo.lock` und `ui/package-lock.json` sind eingecheckt; mit `cargo vendor` und einem
npm-Cache lässt sich ohne Internet bauen. Zur Laufzeit wird nie eine Internetverbindung benötigt.

Optionale Cargo-Features: `--features sm-io/asio` (Windows, ASIO-SDK nötig), `--features sm-io/jack`.

## Pakete

`./packaging/portable.sh` erzeugt die portable Variante (eine ausführbare Datei, UI eingebettet).
`.deb` über `cargo deb -p showmaster`. Stand der übrigen Installer: [`packaging/README.md`](packaging/README.md).

## Lizenz

MIT oder Apache-2.0, nach Wahl.
