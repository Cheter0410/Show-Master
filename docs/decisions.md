# Entscheidungen

Kleinere Entscheidungen, die ohne Rückfrage getroffen wurden – mit Begründung, damit sie sich leicht
revidieren lassen.

## Vorgehen

- **Kein Freigabe-Stopp nach dem Plan.** Der Auftrag lautete, alles ohne Hardware Machbare umzusetzen.
  Architektur und Risiken stehen in `architecture.md`; die drei Risiko-Prototypen (Audio-Streaming,
  LTC mit Decoder, rtp-midi-Handshake) entstanden zuerst und sind als `examples/` erhalten.
- **Offene Rückfragen** (Kanalzahl, LTC/MTC-Eingang, ASIO-Pflicht) sind so entschieden, dass keine
  Antwort etwas verbaut: bis 64+ Kanäle, Timecode-Eingang über `TcTransport` vorbereitet, ASIO als
  Cargo-Feature.

## Stack

- **Desktop: native Rust-UI mit egui (`crates/sm-desktop`); Web nur für Tablets.** Ursprünglich lief
  auch die Desktop-Oberfläche im Browser. Auf Wunsch ist der Desktop jetzt eine native App mit eigenem
  Fenster – dasselbe Toolkit wie Beamweave (egui/eframe 0.37, wgpu). Dateiauswahl über die
  Systemdialoge (rfd: Finder/Explorer/GTK-Portal), Drag & Drop von Dateien mit echten Pfaden.
  Das Fenster ist nur ein weiterer Client am Command-Bus (wie iPad, HTTP-API, OSC); Engine und Audio
  laufen in eigenen Threads, ein hängendes Fenster stört die Wiedergabe nicht. Die Weboberfläche
  (React) bleibt für `/remote` und `/view`; ihre Desktop-Ansicht unter `/` existiert weiter, ist aber
  nicht mehr der Hauptweg. Hotkeys wirken, solange das Fenster den Fokus hat (keine systemweiten
  Hotkeys).
- **cpal reicht** als Audio-Schicht (CoreAudio, WASAPI, ALSA; ASIO und JACK als Feature). Der Mixer
  hängt nur an den Traits `AudioBackend`/`OutputStream`; ein eigenes Backend ließe sich ergänzen.
  PipeWire wird über dessen ALSA- bzw. JACK-Schnittstelle angesprochen. Ob 32+ Kanäle mit niedriger
  Latenz auf allen Plattformen sauber laufen, ist nur an Hardware zu klären.
- **std-Threads + UDP für rtp-midi**, tokio nur für HTTP/WebSocket: Timing-kritisches hängt nicht an
  einem async-Scheduler.
- **Showfile als JSON** (nicht RON): von jedem Werkzeug les- und diffbar.

## Verhalten

- **Disarmed Cues** tun nichts, halten aber die Continue-Kette (wie QLab).
- **Auto-Follow** feuert, wenn die Aktion endet (bei Gruppen: wenn alle Kinder fertig sind);
  **Auto-Continue** nach Pre-Wait + Post-Wait.
- **Ein bereits laufender Cue wird nicht erneut gestartet** (Trigger wird mit Log-Eintrag ignoriert).
- **Kinder einer Gruppe** ignorieren ihren eigenen Continue-Modus; die Gruppe bestimmt die Abfolge.
- **Stop All** blendet mit `stop_fade_ms` aus, **Panic** hart (oder `panic_fade_ms`) und sendet
  All Notes Off / All Sound Off / Reset Controllers auf allen Kanälen aller benutzten Ports.
- **Timecode-Sprünge** je Cue-Liste: *fire* feuert übersprungene Cues (nur vorwärts, nur wenn der
  Generator lief), *skip* nichts, *chase* startet, was an der neuen Position laufen würde, am passenden
  Offset und stoppt Timecode-Cues, deren Trigger in der Zukunft liegt.
- **Fade-Cues** setzen den absoluten Zielpegel des Ziel-Cues; ein laufender Fade lässt sich nicht
  pausieren (die Rampe läuft auf der Master-Clock weiter).
- **Live-Modus:** Edit-Commands werden serverseitig mit `live_mode` abgelehnt. Notizen und
  Arm/Disarm bleiben möglich (gehören zum Betrieb).
- **Loopback ist Admin.** Wer auf dem Show-Rechner selbst sitzt, braucht keinen Login; entfernte
  Clients brauchen eine PIN. Die Verbindung ist unverschlüsseltes HTTP im lokalen Netz – für ein
  abgeschottetes Show-Netz gedacht, nicht für fremde Netze.
- **OSC** ist unauthentifiziert und deshalb standardmäßig aus (`osc_port: 0`), nur Transport-Commands.
- **Port und Bind-Adresse** werden beim Start festgelegt; Änderungen an `remote.port`/`enabled`
  greifen nach einem Neustart.
- **Audio-Einstellungen ändern** stoppt alle Cues und öffnet den Stream neu.
- **Kein Audio-Gerät:** die App fällt auf den stummen Null-Ausgang zurück (Uhr läuft, alles bedienbar)
  und meldet das deutlich im Log und im Pre-Flight.

## Updater (nachträglich auf Wunsch ergänzt)

- **Abweichung von „keine Internetverbindung, keine Telemetrie“:** Auf ausdrücklichen Wunsch gibt es
  einen Updater nach dem Muster von Beamweave. Er ist der einzige Code, der ins Internet geht
  (`crates/sm-server/src/update.rs`; `scripts/offline-test.sh` prüft, dass der HTTP-Client nirgends
  sonst benutzt wird). Er sendet nichts außer der Anfrage an die GitHub-Releases-API.
- Prüfung beim Start (wie Beamweave), abschaltbar mit `--no-update-check` bzw.
  `SHOWMASTER_NO_UPDATE_CHECK=1`. Installation nur auf Knopfdruck und nie im Live-Modus, bei laufenden
  Cues oder ungespeicherter Show.
- Downloads nur von `github.com/Cheter0410/show-master-releases/`, mit Prüfung von Größe und
  SHA-256-Digest; ein Programm-Patch muss wie eine ausführbare Datei des Systems aussehen.
- Schließen des Fensters beendet das Programm. Im Live-Modus (Quit-Schutz), bei laufenden Cues oder
  ungespeicherten Änderungen wird vorher nachgefragt. Der Windows-Release-Build hat kein Konsolenfenster.

## Bekannte Grenzen

- **Dateien werden im Engine-Thread geöffnet.** Durch Standby passiert das vor dem GO; ein Cue, der
  ohne Standby gefeuert wird (Vorschau, Chase), kann auf langsamen Platten wenige Millisekunden später
  einsetzen (als Underrun gezählt).
- **rtp-midi:** kein Bonjour/mDNS (Verbindung per IP:Port), kein empfangsseitiges Recovery, Journal
  (Kapitel P/C/N) standardmäßig aus, bis es gegen echte Gegenstellen geprüft ist.
- **MIDI-Timing** ist nicht samplegenau, sondern über die Audio-Clock terminiert (siehe Architektur).
- **AAC/M4A:** Encoder-Vorlauf wird nicht abgeschnitten (ca. 1024 Frames). Für framegenaue Einsätze
  WAV/AIFF/FLAC verwenden.
- **mDNS** für die Remote-URL gibt es nicht; Zugriff per IP/QR-Code.
