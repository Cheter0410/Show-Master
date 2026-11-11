# Stream Deck / Bitfocus Companion

Show Master braucht kein eigenes Plugin: die lokale HTTP-API reicht für das generische HTTP-Modul von
Companion oder für „Website“/„API Request“-Aktionen der Stream-Deck-Software.

## Befehle

`GET` oder `POST` auf `http://<IP>:7400/api/v1/cmd/<befehl>?token=pin:<Control-PIN>`.
Auf dem Show-Rechner selbst (127.0.0.1) ist kein Token nötig.

| Taste | URL |
|---|---|
| GO | `/api/v1/cmd/go` |
| Panic | `/api/v1/cmd/panic` |
| Stop All | `/api/v1/cmd/stop_all` |
| Pause/Resume | `/api/v1/cmd/toggle_pause` |
| Next / Previous | `/api/v1/cmd/next`, `/api/v1/cmd/previous` |
| GoTo Cue 14 | `/api/v1/cmd/goto_number?number=14` |
| Cue laden | `/api/v1/cmd/load_cue?cue=<id>&time=30` |
| Cue starten (Hot Cue) | `/api/v1/cmd/start_cue?cue=<id>` |
| Arm / Disarm | `/api/v1/cmd/arm_cue?cue=<id>&armed=false` |
| Reset | `/api/v1/cmd/reset` |

Antwort: `{ "ok": true }` oder `{ "ok": false, "error": "…" }`.

## Rückkanal (Button-Feedback)

`GET /api/v1/state?token=…` liefert das Zustandsdokument. Nützliche Felder:

- `playheads.<listId>` → Cue-Id des nächsten Cues; Nummer/Name unter `show.cue_lists[].cues[]`
  (für „Next: Cue 14“)
- `tick.active[]` → laufende Cues mit `phase`, `elapsed`, `duration` (Taste färben, Restzeit)
- `paused_all`, `mode`, `tick.tc.position`, `tick.show_elapsed`

In Companion: Modul *Generic HTTP*, Feedback per JSON-Pfad mit Polling (z. B. 250 ms). Für Feedback
ohne Polling steht der WebSocket `/ws` zur Verfügung (`docs/remote-api.md`).

## OSC

Alternativ `settings.remote.osc_port` setzen (z. B. 7401) und `/showmaster/go`,
`/showmaster/goto_number "14"`, `/showmaster/start_cue 5` senden. OSC hat keine Authentifizierung.
