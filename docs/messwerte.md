# Messwerte

Gemessen in der Entwicklungsumgebung: Linux x86_64, 4 Kerne (VM), **ohne Audio-Gerät** (stummer
Null-Ausgang, der in Echtzeit taktet). Die Zahlen belegen Engine, Mixer, Streaming, Timecode und
Server – nicht das Verhalten eines echten Audio-Interfaces (siehe `hardware-checkliste.md`).

| Messung | Ergebnis | Wie |
|---|---|---|
| Startzeit bis die API antwortet | 29 ms | `scripts/bench-startup.sh` |
| Speicher im Leerlauf | 8,9 MB resident, 1,4 % CPU | dito |
| Größe der ausführbaren Datei (UI eingebettet) | 10 MB | dito |
| Mixer, 48 Stereo-Voices → 64 Ausgänge, offline | 208 ns/Frame = 1,0 % Echtzeit, 0 Underruns | `cargo test --release -p sm-audio --test load` |
| Mixer, 128 Voices → 64 Ausgänge | 802 ns/Frame = 3,9 % Echtzeit | `audio_bench` |
| Allokationen im Audio-Callback | 0 (und 0 Freigaben) über 240 100 Frames | `sm-audio/tests/rt_no_alloc.rs` |
| Versatz gleichzeitig gestarteter Cues | 0 Samples, gemessen am gerenderten Ausgang | `sm-io/tests/sample_accuracy.rs` |
| Auto-Follow / Pre-Wait / Post-Wait | exakt auf das Sample | dito |
| LTC, 1 h bei 25 und 29,97 df | jede Frame-Grenze auf dem Soll-Sample, kein Drift | `sm-timecode`, `no_drift_1_hour_*` |
| Dauertest 30 min: 32 parallele Stereo-Streams alle 5 s, LTC + MTC, 3 Clients, laufende Edits | 0 Underruns, 0 Cue-Fehler, Clock 48 000,0 Frames/s, MTC 100,00 Viertelframes/s, max. 13,8 % CPU | `sm-server/tests/soak.rs` |
| Speicher unter Dauerlast (7 min, ohne Undo-Historie) | 39,5 MB → 39,6 MB | dito |

Anmerkungen:

- Der 30-Minuten-Lauf zeigte 61 → 82 MB. Ursache war der Test selbst (er sammelte alle MIDI-Nachrichten)
  plus die bewusst unbegrenzte Undo-Historie (1790 Edits, je ein Show-Snapshot). Seitdem verwirft der
  Test beides; der Speicher bleibt konstant. Die Undo-Historie wächst im echten Betrieb mit jeder
  Änderung um die Größe der Show und wird beim Laden einer anderen Show geleert.
- Mehrstündige Läufe stehen aus (längster Lauf: 30 min). Die CI kann den Dauertest über
  „Run workflow“ mit frei wählbarer Dauer starten.
- CI: Linux x86_64, Linux aarch64 und Windows sind grün (Format, Clippy, alle Tests, Release-Build,
  portables Paket; unter Linux zusätzlich Offline-Nachweis und Start-Benchmark). Die macOS-Jobs haben
  bisher keinen Runner bekommen und sind daher **nicht gelaufen** – macOS ist ungetestet.
