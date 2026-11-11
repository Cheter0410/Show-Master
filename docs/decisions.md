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

- **UI als eingebettete Web-Oberfläche statt nativer Rust-UI.** Das iPad braucht ohnehin ein
  Web-Frontend; eine einzige Codebasis für Desktop, Remote und View vermeidet doppelte Logik. Die
  Desktop-App ist der Rust-Prozess plus Browserfenster. Eine Tauri-Hülle (natives Fenster, globale
  Hotkeys, Datei-Drop mit Pfaden) ist vorgesehen, aber **nicht gebaut**: sie braucht ein Display zum
  Testen. Folge heute: globale Hotkeys fehlen, Hotkeys wirken im Browserfenster.
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
