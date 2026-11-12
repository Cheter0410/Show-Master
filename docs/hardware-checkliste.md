# Checkliste: an echter Hardware zu prüfen

Diese Punkte konnten in der Entwicklungsumgebung (kein Audio-Gerät, kein MIDI, kein Display) nicht
validiert werden.

## Audio
- [ ] `cargo run --release -p sm-audio --example audio_bench -- --list` zeigt das Interface mit voller Kanalzahl
- [ ] `audio_bench --device default --voices 32` läuft ohne Underruns bei 256 und 128 Frames Puffer
- [ ] Multichannel: Routing auf Kanäle > 2 kommt am richtigen Ausgang an (ALSA direkt, PipeWire, JACK)
- [ ] Linux: PipeWire über ALSA-Plugin und über `--features sm-io/jack`
- [ ] Windows: WASAPI exklusiv/shared, ASIO-Build mit `--features sm-io/asio`
- [ ] macOS: Aggregate Devices, Samplerate-Wechsel im laufenden Betrieb
- [ ] Gerät im Betrieb abziehen: Fehler erscheint im Log, App bleibt bedienbar
- [ ] Gerät wieder einstecken: nach wenigen Sekunden „Reconnecting audio“ im Log, Ton wieder da (Cues neu starten)
- [ ] App ohne Interface starten, Interface einstecken: Umschalten vom stummen Ausgang, sobald kein Cue läuft
- [ ] Gleichzeitig gestartete Cues an einem Messgerät/DAW: Versatz 0 Samples

## Timecode
- [ ] LTC-Ausgang von einem Hardware-Leser / einer DAW gelesen (alle Framerates, Pegel)
- [ ] MTC an einem Empfänger: läuft sauber ein, Relokation per Full Frame
- [ ] 1 h LTC gegen Wordclock-synchronen Leser: kein Drift

## MIDI / rtp-midi
- [ ] System-Ports werden gelistet und geöffnet (CoreMIDI, WinMM, ALSA seq)
- [ ] rtp-midi: Verbindung von macOS (Audio-MIDI-Setup), iOS und rtpMIDI (Windows) – beide Richtungen
- [ ] rtp-midi mit `journal: true` gegen Wireshark-Dissector und echte Empfänger
- [ ] Bonjour: Show Master erscheint in macOS *Audio-MIDI-Setup → Netzwerk* unter „Verzeichnis“, auf
      iOS (z. B. in einer RTP-MIDI-fähigen App) und in rtpMIDI (Windows); Verbinden von dort klappt
- [ ] Bonjour: Sessions von Mac/iPad/rtpMIDI erscheinen in *Setup → MIDI → Found on the network*,
      „Connect“ verbindet; verschwinden nach dem Beenden der Gegenstelle
- [ ] RTP-MIDI über WLAN mit Paketverlust (z. B. `tc qdisc … netem loss 5%` auf einem Linux-Router oder
      Network Link Conditioner auf dem Mac): mit Journal der Gegenstelle bleiben keine Noten hängen,
      Controller/Programm stimmen nach Verlust; Zähler `lost`/`recovered` im Zustand plausibel
- [ ] Empfangsseitiges Recovery gegen das Journal von macOS/iOS (Apple sendet Journale mit eigenen
      Kapiteln; prüfen, dass nichts doppelt oder verspätet auslöst)
- [ ] MSC an einem Lichtpult
- [ ] MIDI-/MTC-Jitter messen, besonders unter macOS (ein virtueller Test-Rechner lieferte eine Nachricht 75 ms zu spät; auf echter Hardware zu prüfen)

## Eingänge (MIDI-Steuerung, MSC, Timecode-Chase)
- [ ] MIDI-Controller (USB) erscheint unter *Setup → MIDI → MIDI input*, „Learn“ übernimmt Note/CC/Program Change, die Zuordnung löst GO / Next / Go to cue aus (CoreMIDI, WinMM, ALSA)
- [ ] Gerät im Betrieb ab- und wieder anstecken: Log „not found“ / „connected“, Zuordnung funktioniert wieder
- [ ] Taster, die nur 127 senden, und Taster mit 127/0: genau ein Auslösen pro Druck
- [ ] MSC von einem Lichtpult (z. B. ETC Eos, grandMA): GO mit Cue-Nummer, GO ohne, STOP, RESUME, ALL_OFF, RESET, LOAD; Device-ID und Command-Format (Sound 0x10 / All 0x7F); Log-Einträge stimmen
- [ ] MIDI über rtp-midi (macOS Audio-MIDI-Setup, iPad) als Eingang mit `rtp:`-Port
- [ ] MTC von einer DAW (Logic, Reaper, Ableton) bei 24/25/29.97df/30: Lock < 0,5 s, Uhr zeigt „EXT MTC LOCKED“, Trigger feuern am richtigen Frame (mit LTC-Ausgang gegen die DAW messen)
- [ ] MTC: Stopp und Locate in der DAW (Full Frame) → Timecode steht an der richtigen Position; Abspielen ab neuer Position → Sprungregel (fire/skip/chase) wie erwartet
- [ ] LTC von Recorder/DAW über einen Audio-Eingang (Pegel −20…0 dBFS, alle Framerates): Lock, Freewheel bei abgezogenem Kabel, „lost“ nach der eingestellten Zeit
- [ ] LTC-Chase 1 h gegen die Quelle: keine doppelten/fehlenden Trigger, Neuverankern (Log „re-anchored“) nur selten; Versatz zwischen Eingangs- und Ausgangsgerät messen und dokumentieren
- [ ] Chase mit LTC-/MTC-Ausgang aktiv: Ausgang folgt der Quelle (Konverter); MTC-Ein- und -Ausgang auf verschiedenen Ports
- [ ] PANIC während Chase: keine weiteren Trigger; „Follow external timecode again“ setzt fort

## Bedienung
- [ ] Oberfläche auf einem echten iPad (Safari, „Zum Home-Bildschirm“, Wake-Lock, Hoch-/Querformat)
- [ ] iPad-Safari öffnet `http://showmaster.local:7400/remote` (Bonjour-Name) im Show-WLAN; mit
      „Client Isolation“/Multicast-Filter im Access Point schlägt das erwartungsgemäß fehl, IP/QR geht
- [ ] Zwei Show-Master-Rechner im selben Netz: der zweite wird zu `showmaster-2.local`, Setup → Remote
      zeigt den richtigen Namen
- [ ] Windows: Bonjour-Dienst (iTunes/rtpMIDI) installiert und nicht installiert – Ankündigung klappt
      in beiden Fällen, Port 5353 wird geteilt
- [ ] Latenz GO vom iPad im WLAN (< 50 ms)
- [ ] Stream Deck über Bitfocus Companion (siehe `streamdeck.md`)
- [ ] Mehrstündiger Dauerlauf: `SM_SOAK_SECONDS=14400 cargo test --release -p sm-server --test soak -- --ignored --nocapture`

## Auslieferung
- [ ] macOS: DMG auf einem echten Mac installieren und starten (Gatekeeper „Dennoch öffnen“, Fenster öffnet sich, Systemdialog zum Öffnen von Dateien, Drag & Drop aus dem Finder, Fenster schließen)
- [ ] macOS: Doppelklick auf eine `.smshow`-Datei im Finder öffnet die Show (bei geschlossener und bei laufender App)
- [ ] Windows/Arch: Windows-Installer (unter Wine gebaut) auf echtem Windows durchklicken, `.smshow` per Doppelklick öffnen
- [ ] Windows: Setup-EXE durchklicken, Startmenü-Eintrag, Doppelklick auf `.smshow`
- [ ] Updater von einer Version auf die nächste, auf macOS und Windows (bisher nur Unit-Tests; es gibt noch kein veröffentlichtes Release)
- [ ] macOS: signieren und notarisieren (Apple-Developer-Zertifikat nötig)
- [ ] Windows: Installer signieren (Code-Signing-Zertifikat nötig)
- [ ] AppImage
