// Setup dialog: audio device, output patch (speaker setup), MIDI, timecode, remote, hotkeys, general.
import { useEffect, useState } from "react";
import { diagnosticsUrl, qrUrl, TC_RATES, type PatchGroup, type PatchOutput, type ShowPatch, type State, type TcJump, type TimerAlarm } from "../api";
import { run } from "../commands";
import { Check, DbField, NumField, Row, Select, TcField, TextField, TimeField } from "../components/fields";
import { Confirm, Modal } from "../components/Modal";
import { ACTION_LABELS, assignHotkey, clearHotkey, comboFromEvent, DEFAULT_HOTKEYS, formatCombo, HOTKEY_ACTIONS, keyForAction, type HotkeyAction } from "../hotkeys";
import { toast, useApp } from "../store";
import { formatWallClock } from "../time";

type Tab = "audio" | "patch" | "midi" | "timecode" | "remote" | "hotkeys" | "general";
const TABS: { id: Tab; label: string }[] = [
  { id: "audio", label: "Audio" }, { id: "patch", label: "Outputs / Patch" }, { id: "midi", label: "MIDI" },
  { id: "timecode", label: "Timecode" }, { id: "remote", label: "Remote" }, { id: "hotkeys", label: "Hotkeys" },
  { id: "general", label: "General" },
];

function us(patch: ShowPatch): void {
  void run({ cmd: "update_show", patch });
}

// ---------------- Audio ----------------

function AudioSetup({ doc }: { doc: State }) {
  const a = doc.show.settings.audio;
  const devices = doc.audio.devices;
  const selected = devices.find((d) => d.id === a.device && (a.backend == null || d.backend === a.backend)) ?? null;
  const effective = selected ?? devices.find((d) => d.is_default) ?? null;
  const rates = effective?.sample_rates.length ? effective.sample_rates : [44100, 48000, 88200, 96000];
  const maxCh = effective?.max_output_channels ?? 64;
  const set = (p: NonNullable<NonNullable<ShowPatch["settings"]>["audio"]>) => us({ settings: { audio: p } });
  const devOptions = [
    { value: -1, label: "System default" },
    ...devices.map((d, i) => ({ value: i, label: `${d.name}  ·  ${d.backend}  ·  ${d.max_output_channels} out${d.is_default ? "  (default)" : ""}` })),
  ];
  return (
    <div className="setup-col">
      <div className={`engine-status${doc.audio.error ? " bad" : doc.audio.is_null ? " warn" : ""}`}>
        <b>{doc.audio.error ? "Audio error" : doc.audio.is_null ? "No audio device – running silent" : "Engine running"}</b>
        <span>{doc.audio.error ?? `${doc.audio.device} · ${doc.audio.backend} · ${doc.audio.sample_rate} Hz · ${doc.audio.buffer_frames} frames · ${doc.audio.channels} outputs`}</span>
      </div>
      <Row label="Device">
        <Select
          value={selected ? devices.indexOf(selected) : -1}
          options={devOptions}
          testId="setup-device"
          onChange={(i) => {
            const d = devices[i];
            set(d ? { backend: d.backend, device: d.id, channels: Math.min(a.channels, d.max_output_channels) } : { backend: null, device: null });
          }}
        />
      </Row>
      <Row label="Sample rate">
        <Select value={a.sample_rate} options={[...new Set([...rates, a.sample_rate])].sort((x, y) => x - y).map((r) => ({ value: r, label: `${r} Hz` }))} onChange={(v) => set({ sample_rate: v })} width={140} />
      </Row>
      <Row label="Buffer" hint="smaller = lower latency, higher risk of dropouts">
        <Select value={a.buffer_frames} options={[...new Set([64, 128, 256, 512, 1024, 2048, a.buffer_frames])].sort((x, y) => x - y).map((r) => ({ value: r, label: `${r} frames (${((r / a.sample_rate) * 1000).toFixed(1)} ms)` }))} onChange={(v) => set({ buffer_frames: v })} width={200} />
      </Row>
      <Row label="Output channels" hint={`device offers up to ${maxCh}`}>
        <NumField value={a.channels} integer min={1} max={maxCh} width={80} testId="setup-channels" onCommit={(v) => set({ channels: v })} />
      </Row>
    </div>
  );
}

// ---------------- Outputs / Patch ----------------

function PatchSetup({ doc }: { doc: State }) {
  const { outputs, groups } = doc.show.patch;
  const channels = Math.max(doc.show.settings.audio.channels, doc.audio.channels, 1);
  const [layout, setLayout] = useState(doc.layouts[0] ?? "stereo");
  const [confirm, setConfirm] = useState(false);
  const setOutputs = (o: PatchOutput[], g: PatchGroup[] = groups) => us({ patch: { outputs: o, groups: g } });
  const setGroups = (g: PatchGroup[]) => us({ patch: { groups: g } });
  const removeOutput = (i: number) => {
    // keep group membership consistent: drop the index and shift the ones above it
    const g = groups.map((gr) => ({ ...gr, outputs: gr.outputs.filter((o) => o !== i).map((o) => (o > i ? o - 1 : o)) }));
    setOutputs(outputs.filter((_, j) => j !== i), g);
  };
  const nextFree = () => {
    for (let c = 0; c < channels; c++) if (!outputs.some((o) => o.channel === c)) return c;
    return 0;
  };
  const chOptions = Array.from({ length: Math.max(channels, ...outputs.map((o) => o.channel + 1)) }, (_, c) => ({ value: c, label: `Device ch ${c + 1}${c >= channels ? " (not available)" : ""}` }));
  return (
    <div className="setup-cols">
      <div className="setup-col">
        <h4>Outputs (speakers)</h4>
        <table className="setup-table" data-testid="patch-outputs">
          <thead>
            <tr><th>#</th><th>Name</th><th>Device channel</th><th /></tr>
          </thead>
          <tbody>
            {outputs.map((o, i) => (
              <tr key={i} className={o.channel >= channels ? "row-warn" : ""}>
                <td className="num">{i + 1}</td>
                <td><TextField value={o.name} ariaLabel={`Output ${i + 1} name`} onCommit={(v) => setOutputs(outputs.map((x, j) => (j === i ? { ...x, name: v } : x)))} /></td>
                <td><Select value={o.channel} options={chOptions} onChange={(v) => setOutputs(outputs.map((x, j) => (j === i ? { ...x, channel: v } : x)))} width={190} /></td>
                <td><button className="btn small" aria-label={`Remove output ${i + 1}`} onClick={() => removeOutput(i)}>✕</button></td>
              </tr>
            ))}
            {outputs.length === 0 ? <tr><td colSpan={4} className="muted">No outputs – add one or apply a layout.</td></tr> : null}
          </tbody>
        </table>
        <div className="setup-actions">
          <button className="btn small" data-testid="patch-add-output" onClick={() => setOutputs([...outputs, { name: `Out ${outputs.length + 1}`, channel: nextFree() }])}>+ Add output</button>
          <span className="spacer" />
          <Select value={layout} options={doc.layouts.map((l) => ({ value: l, label: l }))} onChange={setLayout} width={110} ariaLabel="Layout template" />
          <button className="btn small" onClick={() => setConfirm(true)} disabled={doc.layouts.length === 0}>Apply layout…</button>
        </div>
        <p className="hint">Removing or re-ordering outputs changes what existing cue routings point to – check the Routing tab of your audio cues afterwards.</p>
      </div>
      <div className="setup-col">
        <h4>Groups</h4>
        {groups.length === 0 ? <p className="muted">No groups. Groups are shortcuts for routing mono files to several speakers.</p> : null}
        {groups.map((g, gi) => (
          <div className="group-card" key={gi}>
            <div className="group-head">
              <TextField value={g.name} ariaLabel={`Group ${gi + 1} name`} onCommit={(v) => setGroups(groups.map((x, j) => (j === gi ? { ...x, name: v } : x)))} />
              <Select value={g.layout} options={[...new Set([...doc.layouts, g.layout])].map((l) => ({ value: l, label: l }))} onChange={(v) => setGroups(groups.map((x, j) => (j === gi ? { ...x, layout: v } : x)))} width={100} ariaLabel="Group layout" />
              <button className="btn small" aria-label={`Remove group ${g.name}`} onClick={() => setGroups(groups.filter((_, j) => j !== gi))}>✕</button>
            </div>
            <div className="chips">
              {outputs.map((o, oi) => {
                const on = g.outputs.includes(oi);
                return (
                  <button key={oi} className={`chip${on ? " on" : ""}`} aria-pressed={on} onClick={() => setGroups(groups.map((x, j) => (j === gi ? { ...x, outputs: on ? x.outputs.filter((v) => v !== oi) : [...x.outputs, oi].sort((a, b) => a - b) } : x)))}>
                    {oi + 1} {o.name}
                  </button>
                );
              })}
            </div>
          </div>
        ))}
        <div className="setup-actions">
          <button className="btn small" data-testid="patch-add-group" onClick={() => setGroups([...groups, { name: `Group ${groups.length + 1}`, layout: doc.layouts.includes("stereo") ? "stereo" : doc.layouts[0] ?? "stereo", outputs: [] }])}>+ Add group</button>
        </div>
      </div>
      {confirm ? (
        <Confirm title={`Apply layout “${layout}”?`} confirmLabel="Replace output patch" danger onCancel={() => setConfirm(false)} onConfirm={() => { setConfirm(false); void run({ cmd: "apply_layout", layout }); }}>
          This replaces all outputs and groups by the “{layout}” template. Cue routings may need to be checked afterwards. (Undo works.)
        </Confirm>
      ) : null}
    </div>
  );
}

// ---------------- MIDI ----------------

function MidiSetup({ doc }: { doc: State }) {
  const aliases = doc.show.midi_ports;
  const rtp = doc.show.rtp_midi;
  const ports = doc.midi.ports;
  const [addr, setAddr] = useState("");
  const setAliases = (a: typeof aliases) => us({ midi_ports: a });
  const validAddr = /^[^\s:]+:\d{1,5}$/.test(addr.trim());
  return (
    <div className="setup-cols">
      <div className="setup-col">
        <h4>Port aliases</h4>
        <p className="hint">Cues use the alias, so the show still works when the hardware name changes.</p>
        <table className="setup-table">
          <thead>
            <tr><th>Alias</th><th>Device</th><th /></tr>
          </thead>
          <tbody>
            {aliases.map((p, i) => {
              const missing = p.device !== "" && !ports.includes(p.device);
              return (
                <tr key={i} className={missing ? "row-warn" : ""}>
                  <td><TextField value={p.name} ariaLabel="Alias name" onCommit={(v) => setAliases(aliases.map((x, j) => (j === i ? { ...x, name: v.trim() } : x)))} /></td>
                  <td>
                    <Select value={p.device} options={[{ value: "", label: "— unassigned —" }, ...[...new Set([...ports, ...(missing ? [p.device] : [])])].map((d) => ({ value: d, label: missing && d === p.device ? `${d} (not connected)` : d }))]} onChange={(v) => setAliases(aliases.map((x, j) => (j === i ? { ...x, device: v } : x)))} width={220} />
                  </td>
                  <td><button className="btn small" aria-label={`Remove alias ${p.name}`} onClick={() => setAliases(aliases.filter((_, j) => j !== i))}>✕</button></td>
                </tr>
              );
            })}
            {aliases.length === 0 ? <tr><td colSpan={3} className="muted">No aliases yet.</td></tr> : null}
          </tbody>
        </table>
        <div className="setup-actions">
          <button className="btn small" onClick={() => setAliases([...aliases, { name: `Port ${aliases.length + 1}`, device: ports[0] ?? "" }])}>+ Add alias</button>
          <span className="spacer" />
          <button className="btn small" onClick={() => void run({ cmd: "midi_panic" })}>MIDI panic (all notes off)</button>
        </div>
        <p className="hint">Detected devices: {ports.length ? ports.join(", ") : "none"}</p>
      </div>
      <div className="setup-col">
        <h4>RTP-MIDI (network)</h4>
        <Row label="Enabled"><Check checked={rtp.enabled} onChange={(v) => us({ rtp_midi: { enabled: v } })} label="Run an RTP-MIDI session" /></Row>
        <Row label="Session name"><TextField value={rtp.session_name} onCommit={(v) => us({ rtp_midi: { session_name: v } })} /></Row>
        <Row label="Port"><NumField value={rtp.port} integer min={1024} max={65534} width={90} onCommit={(v) => us({ rtp_midi: { port: v } })} /></Row>
        <Row label="Journal"><Check checked={rtp.journal} onChange={(v) => us({ rtp_midi: { journal: v } })} label="Recovery journal" /></Row>
        <h4>Connect to</h4>
        <ul className="plain-list">
          {rtp.connect_to.map((a, i) => (
            <li key={i}>
              <span className="mono">{a}</span>
              <span className="spacer" />
              <button className="btn small" onClick={() => void run({ cmd: "rtp_connect", addr: a }).then((ok) => ok && toast(`Inviting ${a}…`, "info", 2000))}>Connect</button>
              <button className="btn small" aria-label={`Remove ${a}`} onClick={() => us({ rtp_midi: { connect_to: rtp.connect_to.filter((_, j) => j !== i) } })}>✕</button>
            </li>
          ))}
          <li>
            <input className="input mono" placeholder="192.168.1.20:5004" value={addr} aria-label="Peer address" onChange={(e) => setAddr(e.target.value)} />
            <button className="btn small" disabled={!validAddr || rtp.connect_to.includes(addr.trim())} onClick={() => { us({ rtp_midi: { connect_to: [...rtp.connect_to, addr.trim()] } }); setAddr(""); }}>Add</button>
          </li>
        </ul>
        <h4>Peers</h4>
        <ul className="plain-list" data-testid="rtp-peers">
          {doc.midi.rtp.peers.length === 0 ? <li className="muted">No peers</li> : null}
          {doc.midi.rtp.peers.map((p, i) => (
            <li key={i}>
              <span className={`peer-dot ${p.state === "connected" ? "ok" : ""}`} />
              <b>{p.name}</b>
              <span className="mono muted">{p.addr}</span>
              <span className="spacer" />
              <span>{p.state}</span>
              <span className="num muted">{p.latency_ms != null ? `${p.latency_ms.toFixed(1)} ms` : ""}</span>
            </li>
          ))}
        </ul>
      </div>
    </div>
  );
}

// ---------------- Timecode ----------------

function TimecodeSetup({ doc }: { doc: State }) {
  const tc = doc.show.timecode;
  const set = (p: NonNullable<ShowPatch["timecode"]>) => us({ timecode: p });
  const midiOptions = [{ value: "", label: "— none —" }, ...doc.show.midi_ports.map((p) => ({ value: p.name, label: p.name })), ...doc.midi.ports.filter((d) => !doc.show.midi_ports.some((p) => p.name === d)).map((d) => ({ value: d, label: `${d} (device)` }))];
  const jumpOptions: { value: TcJump; label: string }[] = [
    { value: "fire", label: "Fire – trigger skipped cues" },
    { value: "skip", label: "Skip – ignore skipped cues" },
    { value: "chase", label: "Chase – start cues mid-way" },
  ];
  return (
    <div className="setup-cols">
      <div className="setup-col">
        <h4>Generator</h4>
        <Row label="Frame rate"><Select value={tc.rate} options={TC_RATES.map((r) => ({ value: r, label: r === "29.97df" ? "29.97 drop-frame" : r }))} onChange={(v) => set({ rate: v })} width={170} testId="tc-rate" /></Row>
        <Row label="Start"><TcField value={tc.start} rate={tc.rate} width={140} testId="tc-start" onCommit={(v) => v && set({ start: v })} /></Row>
        <Row label="Mode">
          <Select value={tc.mode} options={[{ value: "freerun" as const, label: "Free run" }, { value: "transport" as const, label: "Follows transport (pause / stop)" }]} onChange={(v) => set({ mode: v })} width={260} />
        </Row>
        <Row label="Test">
          <button className="btn small" onClick={() => void run({ cmd: "tc_start" })}>Start</button>
          <button className="btn small" onClick={() => void run({ cmd: "tc_pause" })}>Pause</button>
          <button className="btn small" onClick={() => void run({ cmd: "tc_stop" })}>Stop</button>
        </Row>
        <h4>When timecode jumps</h4>
        {doc.show.cue_lists.map((l) => (
          <Row label={l.name} key={l.id}>
            <Select value={l.tc_jump} options={jumpOptions} onChange={(v) => void run({ cmd: "update_list", list: l.id, patch: { tc_jump: v } })} width={260} />
          </Row>
        ))}
      </div>
      <div className="setup-col">
        <h4>LTC (audio)</h4>
        <Row label="Enabled"><Check checked={tc.ltc_enabled} onChange={(v) => set({ ltc_enabled: v })} label="Send LTC on an audio output" /></Row>
        <Row label="Device channel"><NumField value={tc.ltc_channel + 1} integer min={1} max={128} width={80} disabled={!tc.ltc_enabled} onCommit={(v) => set({ ltc_channel: Math.max(0, v - 1) })} /></Row>
        <Row label="Level"><DbField value={tc.ltc_level_db} width={80} max={0} disabled={!tc.ltc_enabled} onCommit={(v) => set({ ltc_level_db: v ?? -12 })} /><span className="unit">dBFS</span></Row>
        <h4>MTC (MIDI)</h4>
        <Row label="Enabled"><Check checked={tc.mtc_enabled} onChange={(v) => set({ mtc_enabled: v })} label="Send MIDI timecode" /></Row>
        <Row label="Port"><Select value={tc.mtc_port} options={midiOptions} disabled={!tc.mtc_enabled} onChange={(v) => set({ mtc_port: v })} width={220} /></Row>
      </div>
    </div>
  );
}

// ---------------- Remote ----------------

function PinField({ value, onCommit, label }: { value: string; onCommit: (v: string) => void; label: string }) {
  return (
    <TextField
      value={value}
      width={120}
      ariaLabel={label}
      className="mono"
      onCommit={(v) => {
        const t = v.trim();
        if (!/^\d{0,12}$/.test(t)) toast("A PIN may only contain digits (max. 12)", "warn");
        else onCommit(t);
      }}
    />
  );
}

function RemoteSetup({ doc, token }: { doc: State; token: string }) {
  const r = doc.show.settings.remote;
  const myId = useApp((s) => s.clientId);
  const set = (p: NonNullable<NonNullable<ShowPatch["settings"]>["remote"]>) => us({ settings: { remote: p } });
  return (
    <div className="setup-cols">
      <div className="setup-col">
        <h4>Remote access</h4>
        <Row label="Enabled"><Check checked={r.enabled} onChange={(v) => set({ enabled: v })} label="Allow devices on the local network" /></Row>
        <Row label="Port" hint="applies after restart of the server"><NumField value={r.port} integer min={1024} max={65534} width={90} onCommit={(v) => set({ port: v })} /></Row>
        <Row label="Control PIN" hint={r.control_pin === "" ? "empty = remote control disabled" : "for /remote, Stream Deck, Companion"}><PinField value={r.control_pin} label="Control PIN" onCommit={(v) => set({ control_pin: v })} /></Row>
        <Row label="View PIN" hint="empty = view without PIN"><PinField value={r.view_pin} label="View PIN" onCommit={(v) => set({ view_pin: v })} /></Row>
        <Row label="OSC port" hint="0 = off. OSC has no authentication – trusted networks only."><NumField value={r.osc_port} integer min={0} max={65534} width={90} onCommit={(v) => set({ osc_port: v })} /></Row>
        <h4>Connected clients</h4>
        <table className="setup-table" data-testid="clients">
          <thead><tr><th>Role</th><th>Address</th><th>Since</th><th>Device</th><th /></tr></thead>
          <tbody>
            {doc.remote.clients.map((c) => (
              <tr key={c.id}>
                <td><span className={`role-badge ${c.role}`}>{c.role}</span></td>
                <td className="mono">{c.addr}</td>
                <td className="num">{formatWallClock(c.since_ms)}</td>
                <td className="ellipsis agent" title={c.agent}>{shortAgent(c.agent)}</td>
                <td>{c.id === myId ? <span className="muted">this window</span> : <button className="btn small danger" data-testid="kick" onClick={() => void run({ cmd: "kick_client", id: c.id })}>Disconnect</button>}</td>
              </tr>
            ))}
            {doc.remote.clients.length === 0 ? <tr><td colSpan={5} className="muted">No clients</td></tr> : null}
          </tbody>
        </table>
      </div>
      <div className="setup-col">
        <h4>Open on the iPad</h4>
        {doc.remote.urls.length === 0 ? <p className="muted">No network address found – is this computer connected to the show network?</p> : null}
        <ul className="plain-list urls">
          {doc.remote.urls.map((u) => (
            <li key={u}><span className="mono">{u}/remote</span><span className="muted">control</span></li>
          ))}
          {doc.remote.urls.map((u) => (
            <li key={`${u}v`}><span className="mono">{u}/view</span><span className="muted">view only</span></li>
          ))}
        </ul>
        <div className="qr-row">
          <figure><img src={qrUrl("control", token)} alt="QR code for the control remote" width={150} height={150} /><figcaption>Control (PIN)</figcaption></figure>
          <figure><img src={qrUrl("view", token)} alt="QR code for the view-only page" width={150} height={150} /><figcaption>View only</figcaption></figure>
        </div>
      </div>
    </div>
  );
}

function shortAgent(agent: string): string {
  if (/iPad/.test(agent)) return "iPad";
  if (/iPhone/.test(agent)) return "iPhone";
  if (/Android/.test(agent)) return "Android";
  if (/Macintosh/.test(agent)) return /Safari/.test(agent) && !/Chrome/.test(agent) ? "Mac / iPad Safari" : "Mac";
  if (/Windows/.test(agent)) return "Windows";
  if (/Linux/.test(agent)) return "Linux";
  return agent.slice(0, 40) || "–";
}

// ---------------- Hotkeys ----------------

function HotkeySetup({ doc }: { doc: State }) {
  const hotkeys = doc.show.hotkeys;
  const [capturing, setCapturing] = useState<HotkeyAction | null>(null);
  const [warning, setWarning] = useState<{ action: HotkeyAction; text: string } | null>(null);

  // Merge patch that replaces the whole map: removed keys must be sent as null.
  const commit = (next: Record<string, string>) => {
    const patch: Record<string, string | null> = {};
    for (const k of Object.keys(hotkeys)) if (!(k in next)) patch[k] = null;
    for (const [k, v] of Object.entries(next)) if (hotkeys[k] !== v) patch[k] = v;
    if (Object.keys(patch).length > 0) us({ hotkeys: patch });
  };

  useEffect(() => {
    if (!capturing) return;
    const onKey = (e: KeyboardEvent) => {
      e.preventDefault();
      e.stopPropagation();
      const combo = comboFromEvent(e);
      if (!combo) return; // only modifiers so far
      const res = assignHotkey(hotkeys, capturing, combo);
      if (res.ok) {
        commit(res.hotkeys);
        setWarning(null);
      } else {
        const other = ACTION_LABELS[res.conflict as HotkeyAction] ?? res.conflict;
        setWarning({ action: capturing, text: `${formatCombo(combo)} is already used by “${other}”. Not assigned – clear that binding first.` });
      }
      setCapturing(null);
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [capturing, hotkeys]);

  return (
    <div className="setup-col" data-testid="hotkeys" data-capturing={capturing ?? undefined}>
      {warning ? <div className="hk-warning" role="alert" data-testid="hk-warning">⚠ {warning.text}</div> : null}
      <table className="setup-table hk-table">
        <thead><tr><th>Action</th><th>Key</th><th /></tr></thead>
        <tbody>
          {HOTKEY_ACTIONS.map((a) => {
            const key = keyForAction(hotkeys, a);
            return (
              <tr key={a} className={warning?.action === a ? "row-warn" : ""}>
                <td>{ACTION_LABELS[a]}</td>
                <td>{capturing === a ? <span className="hk-capture">Press a key…</span> : key ? <kbd>{formatCombo(key)}</kbd> : <span className="muted">not set</span>}</td>
                <td className="hk-buttons">
                  <button className={`btn small${capturing === a ? " primary" : ""}`} data-testid={`hk-set-${a}`} onClick={() => { setWarning(null); setCapturing(capturing === a ? null : a); }}>
                    {capturing === a ? "Cancel" : "Change…"}
                  </button>
                  <button className="btn small" disabled={!key} onClick={() => commit(clearHotkey(hotkeys, a))}>Clear</button>
                </td>
              </tr>
            );
          })}
        </tbody>
      </table>
      <div className="setup-actions">
        <button className="btn small" data-testid="hk-reset" onClick={() => { setWarning(null); commit({ ...DEFAULT_HOTKEYS }); }}>Reset to defaults</button>
        <span className="hint">Hotkeys are stored in the show file and work in this desktop window.</span>
      </div>
    </div>
  );
}

// ---------------- General ----------------

function GeneralSetup({ doc, token }: { doc: State; token: string }) {
  const s = doc.show.settings;
  const set = (p: NonNullable<ShowPatch["settings"]>) => us({ settings: p });
  const setAlarms = (a: TimerAlarm[]) => set({ timer_alarms: a });
  return (
    <div className="setup-cols">
      <div className="setup-col">
        <h4>Show</h4>
        <Row label="Show name"><TextField value={doc.show.name} testId="setup-name" onCommit={(v) => us({ name: v })} /></Row>
        <h4>Safety</h4>
        <Row label="GO debounce" hint="a second GO within this time is ignored"><NumField value={s.go_debounce_ms} integer min={0} max={5000} width={90} testId="setup-debounce" onCommit={(v) => set({ go_debounce_ms: v })} /><span className="unit">ms</span></Row>
        <Row label="Stop fade"><NumField value={s.stop_fade_ms} integer min={0} max={60000} width={90} onCommit={(v) => set({ stop_fade_ms: v })} /><span className="unit">ms</span></Row>
        <Row label="Panic fade" hint="0 = cut immediately"><NumField value={s.panic_fade_ms} integer min={0} max={60000} width={90} onCommit={(v) => set({ panic_fade_ms: v })} /><span className="unit">ms</span></Row>
        <Row label="Panic"><Check checked={s.panic_double_tap} onChange={(v) => set({ panic_double_tap: v })} label="Require a double press" testId="setup-panic-double" /></Row>
        <Row label="Double-tap window"><NumField value={s.double_tap_ms} integer min={150} max={3000} width={90} disabled={!s.panic_double_tap} onCommit={(v) => set({ double_tap_ms: v })} /><span className="unit">ms</span></Row>
        <Row label="Quit lock"><Check checked={s.quit_lock} onChange={(v) => set({ quit_lock: v })} label="Ask before closing in Live mode" /></Row>
        <Row label="Display"><Check checked={s.high_contrast} onChange={(v) => set({ high_contrast: v })} label="High contrast" testId="setup-contrast" /></Row>
      </div>
      <div className="setup-col">
        <h4>Show-timer alarms</h4>
        <p className="hint">Shown on all screens when the show clock reaches the time.</p>
        <ul className="plain-list">
          {s.timer_alarms.map((a, i) => (
            <li key={i}>
              <span className="unit">at</span>
              <TimeField value={a.at} width={100} ariaLabel="Alarm time" onCommit={(v) => setAlarms(s.timer_alarms.map((x, j) => (j === i ? { ...x, at: v ?? 0 } : x)))} />
              <TextField value={a.label} ariaLabel="Alarm label" onCommit={(v) => setAlarms(s.timer_alarms.map((x, j) => (j === i ? { ...x, label: v } : x)))} />
              <button className="btn small" aria-label="Remove alarm" onClick={() => setAlarms(s.timer_alarms.filter((_, j) => j !== i))}>✕</button>
            </li>
          ))}
          <li><button className="btn small" data-testid="alarm-add" onClick={() => setAlarms([...s.timer_alarms, { at: 3600, label: "Alarm" }])}>+ Add alarm</button></li>
        </ul>
        <h4>Maintenance</h4>
        <div className="setup-actions">
          <button className="btn small" onClick={() => void run({ cmd: "collect_media" }).then((ok) => ok && toast("Media collected into the show folder", "info"))}>Collect media into show folder</button>
          <a className="btn small" href={diagnosticsUrl(token)} target="_blank" rel="noreferrer">Diagnostics bundle</a>
        </div>
        <p className="hint">Show Master {doc.version}</p>
      </div>
    </div>
  );
}

// ---------------- shell ----------------

export function SetupDialog({ onClose }: { onClose: () => void }) {
  const doc = useApp((s) => s.doc);
  const token = useApp((s) => s.token);
  const [tab, setTab] = useState<Tab>("audio");
  if (!doc) return null;
  const locked = doc.mode === "live";
  return (
    <Modal title="Setup" onClose={onClose} wide className="setup" testId="setup">
      <div className="setup-layout">
        <nav className="setup-tabs" role="tablist" aria-orientation="vertical">
          {TABS.map((t) => (
            <button key={t.id} role="tab" aria-selected={tab === t.id} className={`setup-tab${tab === t.id ? " on" : ""}`} data-testid={`setup-tab-${t.id}`} onClick={() => setTab(t.id)}>
              {t.label}
            </button>
          ))}
        </nav>
        <div className="setup-body">
          {locked ? <div className="hk-warning">Live mode: settings are locked. Switch to Edit to change them.</div> : null}
          <fieldset disabled={locked} className="setup-fieldset">
            {tab === "audio" ? <AudioSetup doc={doc} /> : null}
            {tab === "patch" ? <PatchSetup doc={doc} /> : null}
            {tab === "midi" ? <MidiSetup doc={doc} /> : null}
            {tab === "timecode" ? <TimecodeSetup doc={doc} /> : null}
            {tab === "remote" && token ? <RemoteSetup doc={doc} token={token} /> : null}
            {tab === "hotkeys" ? <HotkeySetup doc={doc} /> : null}
            {tab === "general" && token ? <GeneralSetup doc={doc} token={token} /> : null}
          </fieldset>
        </div>
      </div>
    </Modal>
  );
}
