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
- [ ] Gleichzeitig gestartete Cues an einem Messgerät/DAW: Versatz 0 Samples

## Timecode
- [ ] LTC-Ausgang von einem Hardware-Leser / einer DAW gelesen (alle Framerates, Pegel)
- [ ] MTC an einem Empfänger: läuft sauber ein, Relokation per Full Frame
- [ ] 1 h LTC gegen Wordclock-synchronen Leser: kein Drift

## MIDI / rtp-midi
- [ ] System-Ports werden gelistet und geöffnet (CoreMIDI, WinMM, ALSA seq)
- [ ] rtp-midi: Verbindung von macOS (Audio-MIDI-Setup), iOS und rtpMIDI (Windows) – beide Richtungen
- [ ] rtp-midi mit `journal: true` gegen Wireshark-Dissector und echte Empfänger
- [ ] MSC an einem Lichtpult
- [ ] MIDI-/MTC-Jitter messen, besonders unter macOS (ein virtueller CI-Runner lieferte eine Nachricht 75 ms zu spät; auf echter Hardware zu prüfen)

## Bedienung
- [ ] Oberfläche auf einem echten iPad (Safari, „Zum Home-Bildschirm“, Wake-Lock, Hoch-/Querformat)
- [ ] Latenz GO vom iPad im WLAN (< 50 ms)
- [ ] Stream Deck über Bitfocus Companion (siehe `streamdeck.md`)
- [ ] Mehrstündiger Dauerlauf: `SM_SOAK_SECONDS=14400 cargo test --release -p sm-server --test soak -- --ignored --nocapture`

## Auslieferung
- [ ] macOS: DMG auf einem echten Mac installieren und starten (Gatekeeper „Dennoch öffnen“, Fenster öffnet sich, Systemdialog zum Öffnen von Dateien, Drag & Drop aus dem Finder, Fenster schließen)
- [ ] Windows: Setup-EXE durchklicken, Startmenü-Eintrag, Doppelklick auf `.smshow`
- [ ] Updater von einer Version auf die nächste, auf macOS und Windows (bisher nur Unit-Tests; es gibt noch kein veröffentlichtes Release)
- [ ] macOS: signieren und notarisieren (Apple-Developer-Zertifikat nötig)
- [ ] Windows: Installer signieren (Code-Signing-Zertifikat nötig)
- [ ] AppImage
