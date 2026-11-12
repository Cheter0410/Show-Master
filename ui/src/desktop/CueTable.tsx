// Edit-mode cue table: selection, inline edit, drag & drop reorder, playhead gutter.
import { memo, useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { ActiveCue, Cue, Show } from "../api";
import { run, transport } from "../commands";
import { rowState, StatusIcon } from "../components/CueList";
import { TextField } from "../components/fields";
import { Menu, useLongPress } from "../components/Menu";
import { IconChevronDown, IconChevronRight, IconCopy, IconNext, IconPlay, IconTrash, IconUp, TypeIcon } from "../components/icons";
import { activeMap, CONTINUE_GLYPH, cueCount, cueDuration, cueLabel, cueTarget, findCue, findList, flattenCues, isWithin, stepMove, TYPE_LABELS, type CueRow } from "../cues";
import { deleteCues, duplicateCues, moveCueStep } from "../editing";
import { COARSE_POINTER, isAudioPath, isTauri, onNativeFileDrop, useMedia } from "../platform";
import { currentListId, getApp, setApp, toast, useApp } from "../store";
import { formatDuration, formatSeconds } from "../time";
import { addCue } from "./Toolbar";

type DropPos = "before" | "after" | "into";
interface DropTarget {
  id: number;
  pos: DropPos;
}
const DND_TYPE = "application/x-showmaster-cues";

interface RowProps {
  row: CueRow;
  show: Show;
  active: ActiveCue | undefined;
  error: string | undefined;
  loaded: number | undefined;
  isPlayhead: boolean;
  selected: boolean;
  collapsed: boolean;
  drop: DropPos | null;
  editing: "number" | "name" | null;
  onClick: (id: number, e: React.MouseEvent) => void;
  onToggle: (id: number) => void;
  onEdit: (id: number, field: "number" | "name" | null) => void;
  onDragStart: (id: number, e: React.DragEvent) => void;
  onDragOver: (row: CueRow, e: React.DragEvent) => void;
  onDrop: (e: React.DragEvent) => void;
  onMenu: (id: number, x: number, y: number) => void;
  canDrag: boolean;
}

const EditRow = memo(function EditRow(p: RowProps) {
  const { cue, depth } = p.row;
  const st = rowState(p.active, p.error, p.loaded);
  const dur = cueDuration(cue);
  const cont = CONTINUE_GLYPH[cue.continue_mode];
  const isGroup = cue.kind.type === "group";
  const cls = ["trow", `is-${st}`, p.isPlayhead ? "is-playhead" : "", p.selected ? "is-selected" : "", cue.armed ? "" : "is-disarmed", isGroup ? "is-group" : "", p.drop ? `drop-${p.drop}` : "", cue.color ? `c-${cue.color}` : ""].join(" ");
  const progress = p.active && p.active.duration ? Math.min(1, p.active.elapsed / p.active.duration) : null;
  const longPress = useLongPress((x, y) => p.onMenu(cue.id, x, y));
  return (
    <div
      className={cls}
      role="row"
      aria-selected={p.selected}
      data-cue={cue.id}
      data-state={st}
      data-playhead={p.isPlayhead ? "1" : undefined}
      draggable={p.canDrag && p.editing == null}
      onClick={(e) => p.onClick(cue.id, e)}
      onContextMenu={(e) => {
        if (p.editing != null) return;
        e.preventDefault();
        p.onMenu(cue.id, e.clientX, e.clientY);
      }}
      {...(p.editing == null ? longPress : {})}
      onDragStart={(e) => p.onDragStart(cue.id, e)}
      onDragOver={(e) => p.onDragOver(p.row, e)}
      onDrop={p.onDrop}
    >
      <button
        className="tcell t-gutter"
        title="Set playhead (NEXT) here"
        aria-label={`Set playhead to cue ${cue.number}`}
        data-testid="gutter"
        onClick={(e) => {
          e.stopPropagation();
          void transport.setPlayhead(cue.id);
        }}
      >
        {p.isPlayhead ? <span className="tag tag-next">NEXT</span> : null}
      </button>
      <span className="tcell t-status" title={p.error ?? undefined}>
        <StatusIcon state={st} armed={cue.armed} />
      </span>
      <span className="tcell t-num num" onDoubleClick={() => p.onEdit(cue.id, "number")}>
        {p.editing === "number" ? (
          <TextField value={cue.number} autoFocus ariaLabel="Cue number" onCommit={(v) => void run({ cmd: "update_cue", cue: cue.id, patch: { number: v.trim() } })} onDone={() => p.onEdit(cue.id, null)} />
        ) : (
          cue.number
        )}
      </span>
      <span className="tcell t-type" title={TYPE_LABELS[cue.kind.type]}>
        <TypeIcon type={cue.kind.type} />
      </span>
      <span className="tcell t-name" style={{ paddingLeft: 6 + depth * 18 }} onDoubleClick={() => p.onEdit(cue.id, "name")}>
        {isGroup ? (
          <button
            className="twisty"
            aria-label={p.collapsed ? "Expand group" : "Collapse group"}
            aria-expanded={!p.collapsed}
            onClick={(e) => {
              e.stopPropagation();
              p.onToggle(cue.id);
            }}
            onDoubleClick={(e) => e.stopPropagation()}
          >
            {p.collapsed ? <IconChevronRight size={14} /> : <IconChevronDown size={14} />}
          </button>
        ) : null}
        {p.editing === "name" ? (
          <TextField value={cue.name} autoFocus ariaLabel="Cue name" testId="inline-name" onCommit={(v) => void run({ cmd: "update_cue", cue: cue.id, patch: { name: v } })} onDone={() => p.onEdit(cue.id, null)} />
        ) : (
          <span className="ellipsis t-name-text">{cue.name || <em className="muted">{TYPE_LABELS[cue.kind.type]}</em>}</span>
        )}
      </span>
      <span className={`tcell t-target ellipsis${p.error ? " t-err" : ""}`} title={p.error ?? (cue.kind.type === "audio" ? cue.kind.file : undefined)}>
        {p.error ? p.error : cueTarget(cue, p.show)}
      </span>
      <span className="tcell t-time t-pre num">{formatSeconds(cue.pre_wait)}</span>
      <span className="tcell t-time t-dur num">
        {p.active && p.active.phase === "running" ? (
          <b className="t-run">{p.active.duration != null ? `-${formatDuration(Math.max(0, p.active.duration - p.active.elapsed), false)}` : formatDuration(p.active.elapsed, false)}</b>
        ) : dur != null ? (
          formatDuration(dur)
        ) : cue.kind.type === "audio" && cue.kind.loop_count === 0 ? (
          "∞"
        ) : (
          ""
        )}
      </span>
      <span className="tcell t-time t-post num">{formatSeconds(cue.post_wait)}</span>
      <span className="tcell t-cont" title={cont.title}>{cont.glyph}</span>
      <span className="tcell t-tc num">{cue.tc_trigger ?? ""}</span>
      <span className="tcell t-armed">
        <input
          type="checkbox"
          checked={cue.armed}
          aria-label={`Cue ${cue.number} armed`}
          onClick={(e) => e.stopPropagation()}
          onChange={(e) => void run({ cmd: "update_cue", cue: cue.id, patch: { armed: e.target.checked } })}
        />
      </span>
      {progress != null ? <span className="trow-progress" style={{ width: `${(progress * 100).toFixed(1)}%` }} /> : null}
    </div>
  );
});

const NO_ACTIVE: ActiveCue[] = [];

export function CueTable() {
  const show = useApp((s) => s.doc?.show ?? null);
  const listId = useApp((s) => currentListId(s));
  const playhead = useApp((s) => (listId != null ? s.doc?.playheads[String(listId)] ?? null : null));
  const errors = useApp((s) => s.doc?.errors);
  const loaded = useApp((s) => s.doc?.loaded);
  const active = useApp((s) => s.tick?.active ?? NO_ACTIVE);
  const selection = useApp((s) => s.selection);
  const [collapsed, setCollapsed] = useState<ReadonlySet<number>>(new Set());
  const [editing, setEditing] = useState<{ id: number; field: "number" | "name" } | null>(null);
  const [drop, setDrop] = useState<DropTarget | null>(null);
  const [fileHint, setFileHint] = useState(false);
  const [menu, setMenu] = useState<{ id: number; x: number; y: number } | null>(null);
  const coarse = useMedia(COARSE_POINTER);
  const anchor = useRef<number | null>(null);
  const list = findList(show, listId);
  const rows = useMemo(() => (list ? flattenCues(list.cues, collapsed) : []), [list, collapsed]);
  const rowsRef = useRef(rows);
  rowsRef.current = rows;
  const amap = useMemo(() => activeMap(active), [active]);

  // Keep the (last) selected row visible when the inspector opens below it and shrinks the table.
  const lastSelected = selection[selection.length - 1];
  useEffect(() => {
    if (lastSelected == null) return;
    const raf = requestAnimationFrame(() => document.querySelector(`.cuetable [data-cue="${lastSelected}"]`)?.scrollIntoView({ block: "nearest" }));
    return () => cancelAnimationFrame(raf);
  }, [lastSelected]);

  // Tauri shell: real paths are available for OS file drops.
  useEffect(
    () =>
      onNativeFileDrop((paths) => {
        if (getApp().doc?.mode !== "edit") return;
        void (async () => {
          const audio = paths.filter(isAudioPath);
          if (audio.length === 0) toast("No supported audio files in the drop", "warn");
          for (const f of audio) await addCue("audio", f);
        })();
      }),
    [],
  );

  const handlers = useMemo(() => {
    const onClick = (id: number, e: React.MouseEvent) => {
      const cur = getApp().selection;
      if (e.shiftKey && anchor.current != null) {
        const ids = rowsRef.current.map((r) => r.cue.id);
        const a = ids.indexOf(anchor.current);
        const b = ids.indexOf(id);
        if (a >= 0 && b >= 0) {
          setApp({ selection: ids.slice(Math.min(a, b), Math.max(a, b) + 1) });
          return;
        }
      }
      if (e.ctrlKey || e.metaKey) {
        setApp({ selection: cur.includes(id) ? cur.filter((x) => x !== id) : [...cur, id] });
        anchor.current = id;
        return;
      }
      anchor.current = id;
      if (cur.length !== 1 || cur[0] !== id) setApp({ selection: [id] });
    };
    const onToggle = (id: number) =>
      setCollapsed((c) => {
        const n = new Set(c);
        if (n.has(id)) n.delete(id);
        else n.add(id);
        return n;
      });
    const onEdit = (id: number, field: "number" | "name" | null) => setEditing(field ? { id, field } : null);
    const onDragStart = (id: number, e: React.DragEvent) => {
      let sel = getApp().selection;
      if (!sel.includes(id)) {
        sel = [id];
        anchor.current = id;
        setApp({ selection: sel });
      }
      e.dataTransfer.setData(DND_TYPE, JSON.stringify(sel));
      e.dataTransfer.setData("text/plain", `${sel.length} cue(s)`);
      e.dataTransfer.effectAllowed = "move";
    };
    const onDragOver = (row: CueRow, e: React.DragEvent) => {
      if (!e.dataTransfer.types.includes(DND_TYPE)) return;
      e.preventDefault();
      e.dataTransfer.dropEffect = "move";
      const rect = e.currentTarget.getBoundingClientRect();
      const f = (e.clientY - rect.top) / rect.height;
      const isGroup = row.cue.kind.type === "group";
      const pos: DropPos = isGroup ? (f < 0.25 ? "before" : f > 0.75 ? "after" : "into") : f < 0.5 ? "before" : "after";
      setDrop((d) => (d && d.id === row.cue.id && d.pos === pos ? d : { id: row.cue.id, pos }));
    };
    const onMenu = (id: number, x: number, y: number) => {
      // act on the selection when the row is part of it, otherwise on this row alone
      if (!getApp().selection.includes(id)) {
        anchor.current = id;
        setApp({ selection: [id] });
      }
      setMenu({ id, x, y });
    };
    return { onClick, onToggle, onEdit, onDragStart, onDragOver, onMenu };
  }, []);

  const onDrop = (e: React.DragEvent) => {
    const target = drop;
    setDrop(null);
    if (!e.dataTransfer.types.includes(DND_TYPE) || !target || !show || !list) return;
    e.preventDefault();
    let ids: number[];
    try {
      ids = JSON.parse(e.dataTransfer.getData(DND_TYPE)) as number[];
    } catch {
      return;
    }
    const all = flattenCues(list.cues);
    const cues = ids.map((id) => findCue(show, id)).filter((c): c is Cue => c != null);
    // Only move the top-most selected cues (children travel with their group), in list order.
    const top = cues.filter((c) => !cues.some((o) => o !== c && isWithin(o, c.id)));
    const order = new Map(all.map((r, i) => [r.cue.id, i]));
    top.sort((a, b) => (order.get(a.id) ?? 0) - (order.get(b.id) ?? 0));
    const t = all.find((r) => r.cue.id === target.id);
    if (!t || top.length === 0) return;
    if (top.some((c) => isWithin(c, t.cue.id))) {
      if (!top.some((c) => c.id === t.cue.id)) toast("A group cannot be moved into itself", "warn");
      return;
    }
    const parent = target.pos === "into" ? t.cue.id : t.parent;
    const index = target.pos === "into" ? (t.cue.kind.type === "group" ? t.cue.kind.children.length : 0) : t.index + (target.pos === "after" ? 1 : 0);
    void run({ cmd: "move_cues", cues: top.map((c) => c.id), list: list.id, parent, index });
  };

  const onDropRef = useRef(onDrop);
  onDropRef.current = onDrop;
  const onDropStable = useCallback((e: React.DragEvent) => onDropRef.current(e), []);

  const onTableDragOver = (e: React.DragEvent) => {
    if (e.dataTransfer.types.includes("Files") && !e.dataTransfer.types.includes(DND_TYPE)) {
      e.preventDefault();
      e.dataTransfer.dropEffect = "none";
      if (!isTauri()) setFileHint(true);
    }
  };

  if (!show || !list) return <div className="cuetable empty">No cue list</div>;
  return (
    <div
      className="cuetable"
      data-testid="cuetable"
      onDragOver={onTableDragOver}
      onDragLeave={(e) => {
        if (!e.currentTarget.contains(e.relatedTarget as Node | null)) {
          setDrop(null);
          setFileHint(false);
        }
      }}
      onDrop={(e) => {
        if (e.dataTransfer.types.includes("Files")) {
          e.preventDefault();
          setFileHint(false);
          if (!isTauri()) toast("The browser does not reveal file paths – use “Add cue → Audio…” to pick the file on the show computer.", "warn", 7000);
        }
        setDrop(null);
      }}
      onDragEnd={() => setDrop(null)}
    >
      <div className="thead" role="row">
        <span className="tcell t-gutter" />
        <span className="tcell t-status" />
        <span className="tcell t-num">#</span>
        <span className="tcell t-type" />
        <span className="tcell t-name">Name</span>
        <span className="tcell t-target">Target / file</span>
        <span className="tcell t-time t-pre">Pre</span>
        <span className="tcell t-time t-dur"><span className="th-long">Duration</span><span className="th-short">Time</span></span>
        <span className="tcell t-time t-post">Post</span>
        <span className="tcell t-cont" title="Continue mode">↓</span>
        <span className="tcell t-tc">TC trigger</span>
        <span className="tcell t-armed">Arm</span>
      </div>
      <div
        className="tbody"
        role="grid"
        aria-multiselectable="true"
        onClick={(e) => {
          if (e.target === e.currentTarget && getApp().selection.length) setApp({ selection: [] });
        }}
      >
        {rows.length === 0 ? <div className="cuelist-empty">No cues yet – use “Add cue” above.</div> : null}
        {rows.map((r) => (
          <EditRow
            key={r.cue.id}
            row={r}
            show={show}
            active={amap.get(r.cue.id)}
            error={errors?.[String(r.cue.id)]}
            loaded={loaded?.[String(r.cue.id)]}
            isPlayhead={playhead === r.cue.id}
            selected={selection.includes(r.cue.id)}
            collapsed={collapsed.has(r.cue.id)}
            drop={drop && drop.id === r.cue.id ? drop.pos : null}
            editing={editing && editing.id === r.cue.id ? editing.field : null}
            onClick={handlers.onClick}
            onToggle={handlers.onToggle}
            onEdit={handlers.onEdit}
            onDragStart={handlers.onDragStart}
            onDragOver={handlers.onDragOver}
            onDrop={onDropStable}
            onMenu={handlers.onMenu}
            canDrag={!coarse}
          />
        ))}
      </div>
      {menu ? <RowMenu id={menu.id} x={menu.x} y={menu.y} onClose={() => setMenu(null)} /> : null}
      {fileHint ? (
        <div className="file-hint" role="status">
          Files cannot be dropped from this computer: the browser hides their real paths.
          <br />
          Use <b>Add cue → Audio…</b> to choose the file on the show machine.
        </div>
      ) : null}
    </div>
  );
}

/** Context menu of a cue row (right-click, or long-press on touch screens). */
function RowMenu({ id, x, y, onClose }: { id: number; x: number; y: number; onClose: () => void }) {
  const show = useApp((s) => s.doc?.show ?? null);
  const listId = useApp((s) => currentListId(s));
  const selection = useApp((s) => s.selection);
  const cue = findCue(show, id);
  if (!cue) return null;
  const ids = selection.includes(id) ? selection : [id];
  const many = ids.length > 1;
  const list = findList(show, listId);
  const items = [
    { label: "Set NEXT (playhead) here", icon: <IconNext />, onSelect: () => void transport.setPlayhead(id), testId: "ctx-playhead", disabled: many },
    { label: "Preview (start this cue)", icon: <IconPlay />, onSelect: () => void transport.startCue(id), testId: "ctx-preview", disabled: many },
    { label: "Move up", icon: <IconUp />, onSelect: () => void moveCueStep(id, -1), testId: "ctx-up", disabled: many || !stepMove(list, id, -1), separator: true },
    { label: "Move down", icon: <IconChevronDown />, onSelect: () => void moveCueStep(id, 1), testId: "ctx-down", disabled: many || !stepMove(list, id, 1) },
    { label: many ? `Duplicate ${cueCount(ids.length)}` : "Duplicate", icon: <IconCopy />, onSelect: () => void duplicateCues(ids), testId: "ctx-duplicate", separator: true },
    { label: many ? `Delete ${cueCount(ids.length)}` : "Delete", icon: <IconTrash />, onSelect: () => void deleteCues(ids), testId: "ctx-delete", danger: true },
  ];
  return <Menu at={{ x, y }} items={items} onClose={onClose} heading={many ? `${cueCount(ids.length)} selected` : cueLabel(cue)} testId="row-menu" />;
}
