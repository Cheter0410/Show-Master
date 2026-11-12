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
  nicht mehr der Hauptweg. Hotkeys wirken, solange das Fenster den Fokus hat; systemweite Hotkeys
  sind optional pro Rechner einstellbar (*Setup → Hotkeys*).
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
  Ein laufender Timecode-Generator wird dabei **angehalten** (auch im Freerun-Modus), damit weder
  LTC/MTC weiterlaufen noch Timecode-Trigger erneut Cues feuern; die Position bleibt stehen. Fortsetzen
  im Live-Dashboard per Rechtsklick/langem Druck auf die Timecode-Uhr. *Stop All* lässt einen
  Freerun-Timecode weiterlaufen.
- **Zusätzliche PANIC-Nachrichten** (*Setup → General → Also send on PANIC*): beliebig viele MIDI- oder
  OSC-Nachrichten, gesendet direkt nach All Notes Off – z. B. Blackout ans Lichtpult. Show Master weiß
  sonst nichts von DMX, Video oder anderen Geräten; nur was hier steht, wird mitgestoppt.
- **HTTP-Cues** senden eine einfache HTTP/1.1-Anfrage (GET/POST/PUT/PATCH/DELETE) über eine eigene
  TCP-Verbindung (kein HTTP-Client-Crate; der bleibt dem Updater vorbehalten). Nur `http://`: Show-Netze
  sind lokal, Geräte haben selten gültige Zertifikate. Jede Anfrage läuft auf einem eigenen kurzen Thread,
  ein hängendes Gerät bremst nichts; Fehler stehen im Log.
- **OSC-Cues** senden eine OSC-1.0-Nachricht per UDP (keine Bundles, kein TCP), über denselben
  Sender-Thread wie MIDI und damit genauso auf die Audio-Clock terminiert. Hostnamen werden einmal
  aufgelöst und gemerkt; Zieladressen sind Sache des Nutzers (das ist bewusster Netzwerkverkehr, für das
  Offline-Prinzip zählt: ohne OSC-Cue wird nichts gesendet).
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
- **Fenster schließen ≠ beenden.** Das Schließen-Kreuz (macOS auch Cmd+W) versteckt das Fenster nur;
  Engine, Audio, Timecode, MIDI und der Tablet-Server laufen im selben Prozess weiter. Ein Prozess
  statt getrenntem Server + Fenster-Programm: keine Absprache zwischen zwei Programmen, nichts kann
  auseinanderlaufen. Zurück kommt das Fenster durch erneutes Starten (der zweite Start meldet
  `raise_window` an den laufenden und beendet sich), über das Tray-Symbol (macOS: Menüleiste) und auf
  macOS über das Dock bzw. Finder (Apple Event „reopen“). Beim ersten Schließen erklärt ein Hinweis das
  Verhalten (einmal pro Rechner, `desktop.json`).
- **Beenden** nur über *Quit* im Fenster (macOS Cmd+Q – das eigene App-Menü ersetzt das von winit, dessen
  Quit den Prozess ohne Rückfrage beenden würde) oder *Quit Show Master* im Tray-Menü. Im Live-Modus
  (Quit-Schutz), bei laufenden Cues oder ungespeicherten Änderungen wird vorher nachgefragt (dazu kommt
  das Fenster nach vorne). Remote-`quit` und das Update-Installieren beenden wie bisher direkt.
- **Tray:** macOS/Windows über `tray-icon`, Linux über StatusNotifierItem (`ksni`, reines D-Bus, kein
  GTK-Hauptloop und kein libappindicator nötig: KDE, Xfce, Cinnamon, GNOME mit AppIndicator-Erweiterung).
  Ohne Tray (bzw. unter Wayland, das keine versteckten Fenster kennt) wird das Fenster beim Schließen nur
  minimiert, damit es nie unauffindbar im Hintergrund läuft.
- Bei verstecktem oder minimiertem Fenster drosselt eframe die Logik-Durchläufe auf etwa 10/s. Damit
  systemweite Hotkeys trotzdem sofort wirken, schickt der Hotkey-Handler GO, PANIC, Stop all und Pause
  in diesem Zustand direkt an die Runtime (mit GO-Entprellung und PANIC-Doppeldruck); die übrigen
  Aktionen laufen weiter über die UI und können bis zu ~0,1 s später kommen. Der Windows-Release-Build hat kein Konsolenfenster.

## mDNS/Bonjour und RTP-MIDI-Recovery (nachträglich)

- **mDNS ist eine optionale Komfortfunktion mit Rückfallebene.** Show Master meldet die Weboberfläche
  als `_http._tcp` („Show Master on <Rechner>“, TXT `path=/remote`) unter dem Namen
  `showmaster.local` an (bei Konflikt mit einem zweiten Show-Master-Rechner automatisch
  `showmaster-2.local`) und die RTP-MIDI-Session als `_apple-midi._udp` auf ihrem Control-Port; andere
  RTP-MIDI-Sessions werden gesucht und in *Setup → MIDI* mit „Connect“ angeboten. Zugriff per IP und
  QR-Code bleibt der Hauptweg und funktioniert immer – auch wenn Multicast im Netz gesperrt ist,
  Port 5353 nicht geöffnet werden kann oder der Dienst nicht startet (dann nur Log-Eintrag und Hinweis
  in Setup, kein Fehler).
- **Kurze Adresse `showmaster.local`:** Damit auf dem Handy nur der Name getippt werden muss, lauscht
  der Webserver im Netzwerkbetrieb zusätzlich auf Port 80 (Best Effort, neben dem eingestellten Port),
  und `/` leitet Geräte aus dem Netz per 302 auf die Anzeige `/view` um (sicherer Standard: wer nur
  den Namen tippt, kann nichts auslösen; der Knopf „Control“ führt mit Control-PIN zu `/remote`) (der eigene Rechner bekommt weiter die
  volle Oberfläche). Angekündigt und angezeigt (Setup → Remote, Statuszeile, QR-Codes) wird dann die
  Adresse ohne Port. macOS und Windows erlauben Port 80 ohne Rechte (Windows fragt ggf. die Firewall);
  unter Linux setzen Arch-Paket und .deb `cap_net_bind_service` per `setcap`. Fehlt die Berechtigung
  (z. B. tar.gz/AppImage, oder nach einem Programm-Update, das die Datei ersetzt), bleibt alles beim
  eingestellten Port: `showmaster.local:7400`.
- **Datenschutz/Offline:** mDNS sendet ausschließlich an die Link-lokalen Multicast-Gruppen
  224.0.0.251 / ff02::fb, UDP 5353 (TTL 255, Router leiten das nicht weiter). Angekündigt werden nur
  Rechnername, Instanzname, IP-Adressen und Ports – keine Show-Inhalte. `scripts/offline-test.sh`
  erlaubt genau diese Pakete und sonst weiterhin nichts außer Loopback.
- **Schalter:** `settings.remote.mdns` (Weboberfläche, Standard an) und `rtp_midi.advertise`
  (RTP-MIDI ankündigen *und* suchen, Standard an). Beide wirken ohne Neustart (ein Hintergrund-Thread
  gleicht jede Viertelsekunde mit den Einstellungen ab); sind beide aus, läuft kein mDNS-Dienst und
  es wird nichts gesendet. Mit `--local-only` oder ausgeschaltetem Remote wird die Weboberfläche nicht
  angekündigt. Auch `--headless` kündigt an.
- **Bibliothek:** `mdns-sd` (reines Rust, eigener Thread, keine async-Runtime), Version fest gepinnt.
  Der mDNS-Thread liest nur den veröffentlichten Zustand und blockiert weder Engine noch Audio.
- **Recovery beim Empfang (RFC 6295 §4, Anhang A):** Bei einer Lücke in der Sequenznummer und J=1 im
  nächsten Paket vergleicht der Empfänger das Journal mit seinem Wissen über den Strom des Peers
  (Programm/Bank, Controller-Werte, klingende Noten je Kanal) und liefert die Differenz **vor** den
  Befehlen des Pakets aus: Note-Offs für Noten, die laut Journal losgelassen wurden, abweichende
  Controller- und Programmwerte, und Note-Ons nur, wenn der Sender sie als frisch markiert (Y=1) –
  alte Trigger werden nie nachträglich ausgelöst. Bei genau einem verlorenen Paket werden nur
  Strukturen mit S=0 (= das verlorene Paket) ausgewertet. Das Journal des ersten Pakets einer
  Verbindung wird bewusst nicht ausgewertet (keine Vorgeschichte, sonst würden alte Befehle feuern).
  Funktioniert unabhängig davon, ob wir selbst mit Journal senden. Zähler `lost`/`recovered` je Peer
  im Zustand.

## Robustheit (nachträglich)

- **Audio-Gerät verloren / zurück:** Ein Watcher-Thread hält die Geräteliste aktuell. Fällt das Gerät aus
  und ist es wieder da, wird neu verbunden (alle Cues stoppen dabei – sie waren ohnehin stumm). Läuft die
  App mangels Gerät auf dem stummen Ausgang, wird umgeschaltet, sobald ein Gerät da ist und **kein Cue
  läuft**. Wiederholte Fehlversuche warten länger (bis 60 s).
- **Undo:** die letzten 500 Schritte (jeder Schritt ist eine Kopie der Show).
- **Fenster im Leerlauf:** 4 Bilder/s statt 25, sobald nichts läuft und kein Pegel anliegt.
- **Keine CI:** Gebaut wird lokal (Linux-PC: Linux + Windows, Mac: DMG). Die GitHub-Workflows wurden
  entfernt, damit keine Actions-Kosten entstehen.

## Externe Eingänge: MIDI-Steuerung, MSC, Timecode-Chase (nachträglich)

- **Nichts reagiert ungefragt.** MIDI-Eingang steht standardmäßig auf „Nothing“, die Zuordnungsliste
  ist leer und MSC-Empfang aus. Erst was in *Setup → MIDI* eingerichtet ist, löst etwas aus. „All
  ports“ lässt Ports mit „Through“ im Namen aus (Loopback: eigene MIDI-Cues kämen sonst als Eingang
  zurück); ausdrücklich ausgewählt werden können sie trotzdem.
- **Ein Weg für alles.** Zugeordnete Nachrichten und MSC werden zu normalen Transport-Commands (Rolle
  *control*, Herkunft `midi`) und laufen über denselben Command-Bus wie Hotkeys und iPad, inklusive
  GO-Entprellung. Jede ausgelöste Zuordnung und jede MSC-Nachricht an uns steht im Log.
- **Controller (CC) lösen auf der steigenden Flanke aus** (Wert erreicht oder überschreitet
  `min_value`); ein erneuter hoher Wert gilt nach 250 ms Pause als neuer Tastendruck (Taster, die nur
  127 senden). Fader gehören nicht auf GO. Note-on mit Velocity 0 ist Note-off und löst nie aus.
- **MSC:** `GO` mit Cue-Nummer = Playhead auf den Cue und GO (wie QLab); liegt der Cue in einer Gruppe,
  wird er direkt gestartet. `STOP`/`RESUME` mit Nummer betreffen nur diesen Cue, ohne Nummer alles.
  `Q_list`/`Q_path` werden ignoriert (Cue-Nummern sind showweit eindeutig). `TIMED_GO`, `SET`, `FIRE`,
  `GO_OFF`, `RESTORE` werden protokolliert und ignoriert. Device-ID 127 in den Einstellungen = auf jede
  ID antworten; All-Call (127) und Format 0x7F werden immer angenommen.
- **Empfang blockiert nichts.** Ports öffnet ein eigener Thread (`sm-midi-in`, Rescan alle 2 s,
  ausgesteckte Geräte werden geschlossen und beim Wiederkommen neu geöffnet); der MIDI-Callback stempelt
  nur die Zeit und legt die Nachricht in einen Kanal. Die LTC-Eingabe läuft über einen eigenen
  cpal-Eingangsstrom (`sm_audio::input`): Der Callback kopiert einen Kanal in einen lock-freien
  Ringpuffer (2 s) – ohne Allokation, Lock oder I/O –, öffnen/schließen erledigt der Thread
  `sm-audio-in`, dekodiert wird auf dem Engine-Thread.
- **Chase-Verhalten:**
  - *Zeitbezug:* Jedes empfangene Frame wird auf die Audio-Clock abgebildet: MTC über den
    Empfangszeitpunkt des Viertelframes 0 (Beginn des gemeldeten Frames; Empfänger rechnen „+2 Frames“
    beim kompletten Zyklus), LTC über das Ende des Sync-Worts (Beginn des nächsten Frames) und die
    Zeitstempel der Eingangs-Callbacks. Der Engine-Transport wird auf genau diesen Sample-Zeitpunkt
    verankert; von dort rechnen Trigger, LTC- und MTC-Ausgang mit derselben Ganzzahlarithmetik wie beim
    Generator.
  - *Lock* nach `lock_frames` (Standard 4) aufeinanderfolgenden, zueinander passenden Frames (±½ Frame).
  - *Abweichung:* bis 1 Frame wird toleriert (Jitter), darüber muss ein zweites Frame die neue Lage
    bestätigen. Bis 4 Frames gilt das als Drift: neu verankern, schon gefeuerte Trigger feuern nicht
    erneut, übersprungene sofort. Darüber ist es ein **Sprung**: die Sprungregel der Cue-Liste
    (fire/skip/chase) gilt wie beim Generator. Ein einzelnes Ausreißer-Frame ändert nichts.
  - *Freewheel:* Bleibt das Signal aus (> 6 Frames + 50 ms), läuft der Timecode selbständig weiter
    (Trigger feuern weiter) bis `freewheel_ms` (Standard 2000 ms) nach dem letzten gültigen Frame;
    dann **lost**: der Timecode bleibt an der Position stehen, kein Trigger feuert mehr. Der Verlust ist
    ein Zeitpunkt in der Engine (unabhängig von der Schrittweite).
  - *Stopp:* MTC-Full-Frame (Quelle steht oder springt) und ein LTC-Standbild (dasselbe Frame dreimal)
    halten den Timecode sofort an der gemeldeten Position an. Läuft die Quelle wieder, wird neu gelockt
    – als Sprung mit der Sprungregel der Liste (*fire* feuert dabei nichts, weil vorher nichts lief).
  - *Rückwärts* laufendes MTC/LTC wird nicht verfolgt (gilt als Aussetzer).
  - *PANIC* hält das Verfolgen an („held“), damit keine Trigger mehr feuern; Fortsetzen per
    Rechtsklick auf die Timecode-Uhr („Follow external timecode again“) oder `tc_resume`. Timecode-Cues,
    `tc_start`/`tc_stop` und Transport-Modus bewegen den Timecode im Chase-Betrieb nicht.
  - *Genauigkeit:* Trigger feuern auf dem Sample, das der Verankerung entspricht. Mit LTC ist das im
    Test samplegenau zur Quelle (±2 Samples, gleiche Clock); real kommen der Versatz zwischen Eingangs-
    und Ausgangsgerät (Puffer, Wandlerlatenz) und bei MTC der Empfangsjitter des Betriebssystems hinzu –
    dokumentierte Toleranz: ±1 Frame, typisch wenige Millisekunden (an Hardware zu messen).
- **LTC/MTC-Ausgang im Chase-Betrieb** senden den verfolgten Timecode neu (Konverter MTC↔LTC; bei
  Freewheel läuft er weiter, bei *lost* steht er). Pre-Flight warnt, wenn MTC über denselben Port
  empfangen und gesendet wird (Rückkopplung), und meldet fehlende Eingangs-Ports/-Geräte.
- **Kein Schema-Sprung.** Alle neuen Felder (`midi_input`, `timecode.source`, `timecode.input`) haben
  Standardwerte; Version-2-Shows ohne sie laden unverändert (Test in `showfile.rs`).

## Bekannte Grenzen

- **Dateien öffnen im Hintergrund.** Audiodateien werden von zwei eigenen Threads geöffnet, nie von der
  Engine. Durch Standby passiert das vor dem GO; ein Cue ohne Standby (Vorschau, Chase, Control-Cue)
  beginnt, sobald die Datei offen ist – eine Verspätung über 10 ms steht im Log.
- **rtp-midi:** Senden mit Journal (Kapitel P/C/W/N/T/A und System-Kapitel X für SysEx) standardmäßig
  aus, bis es gegen echte Gegenstellen geprüft ist. Empfangsseitiges Recovery stellt Programm,
  Controller, Noten, Pitch Bend, Channel- und Poly-Aftertouch wieder her; SysEx (z. B. MSC GO) nur, wenn
  sicher ist, dass es im verlorenen Paket stand – lieber ein verpasstes als ein doppeltes GO. Nicht
  ausgewertet: Kapitel M (RPN/NRPN), E und die System-Kapitel D/V/Q/F; das Journal des ersten Pakets
  einer Verbindung wird nicht ausgewertet. Gegen Apple/rtpMIDI noch nicht geprüft.
- **MIDI-Timing** ist nicht samplegenau, sondern über die Audio-Clock terminiert (siehe Architektur).
- **Timecode-Eingang:** Die Rate der Quelle wird erkannt (MTC: Rate-Code; LTC: Abstand der Frames plus
  Drop-Frame-Flag) und nach Timecode-*Wert* auf die Show-Rate umgerechnet (01:00:10:15 bei 30 fps =
  01:00:10:12 bei 25 fps) – so verhalten sich übliche Chase-Systeme; 29,97 ND und 30 bzw. 23,976 und 24
  lassen sich an den Labels nicht unterscheiden und werden gleich behandelt. Varispeed: Ausgleichsgerade
  über die letzten Frames, Abweichung > ¼ Frame wird nachgezogen (getestet ±3 %), die gemessene
  Geschwindigkeit steht in Setup. Rückwärtslauf (LTC rückwärts dekodiert, MTC mit fallenden Frames):
  Uhr steht und folgt der Position, nichts feuert; vorwärts wird neu gelockt (Sprung-Regeln gelten).
  Latenz des Eingangswegs einstellbar (`latency_ms`); der Wert muss am echten Gerät gemessen werden.
- **AAC/M4A:** Encoder-Vorlauf und Auffüllung am Ende werden anhand von `iTunSMPB` (Apple/iTunes) bzw.
  der Edit-List der Tonspur (ffmpeg u. a.) abgeschnitten, damit startet der Ton framegenau. Dateien ohne
  beide Angaben (rohe `.aac`-ADTS-Streams) behalten den Vorlauf (~1024 Frames) – für framegenaue
  Einsätze dann WAV/AIFF/FLAC oder M4A verwenden. MP3: LAME-Gapless-Angaben wertet symphonia aus.
- **mDNS/Bonjour:** nur IPv4-Erkennung; Android-Tablets lösen `.local`-Namen oft nicht auf, und
  WLANs mit Client-Isolation blockieren Multicast – dann gilt die IP-Adresse bzw. der QR-Code.
- **Systemweite Hotkeys:** unter Linux nur mit X11 (nicht Wayland); Tastendruck kommt dort mit bis zu
  50 ms Verzögerung an. Eine per Programm-Update aktualisierte, Developer-ID-signierte Mac-App wird ad hoc
  neu signiert (läuft weiter, verliert aber die Developer-ID-Signatur; das DMG-Update behält sie).
