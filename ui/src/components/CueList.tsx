// Live cue list shared by the desktop dashboard, /remote and /view.
import { memo, useEffect, useMemo, useRef } from "react";
import type { ActiveCue, Cue, Show } from "../api";
import { activeMap, CONTINUE_GLYPH, cueDuration, cueTarget, findList, flattenCues, TYPE_LABELS } from "../cues";
import { currentListId, setApp, useApp } from "../store";
import { formatCountdown, formatDuration } from "../time";
import { IconDisarmed, IconError, IconLoaded, IconPause, IconPlay, TypeIcon } from "./icons";

export type RowState = "idle" | "running" | "paused" | "pre_wait" | "post_wait" | "error" | "loaded";

export function rowState(active: ActiveCue | undefined, error: string | undefined, loaded: number | undefined): RowState {
  if (active) {
    if (active.paused) return "paused";
    return active.phase === "running" ? "running" : active.phase;
  }
  if (error) return "error";
  if (loaded != null) return "loaded";
  return "idle";
}

export function StatusIcon({ state, armed, size = 16 }: { state: RowState; armed: boolean; size?: number }) {
  switch (state) {
    case "running":
    case "post_wait":
      return <span className="st st-running"><IconPlay size={size} title="Running" /></span>;
    case "pre_wait":
      return <span className="st st-running"><span className="st-dot" title="Pre-wait" /></span>;
    case "paused":
      return <span className="st st-paused"><IconPause size={size} title="Paused" /></span>;
    case "error":
      return <span className="st st-error"><IconError size={size} title="Broken" /></span>;
    case "loaded":
      return <span className="st st-loaded"><IconLoaded size={size} title="Loaded" /></span>;
    default:
      return armed ? <span className="st" /> : <span className="st st-disarmed"><IconDisarmed size={size} title="Disarmed" /></span>;
  }
}

/** Remaining / elapsed text and progress fraction of an active cue. */
export function activeTiming(a: ActiveCue): { text: string; label: string; progress: number | null } {
  if (a.duration == null) return { text: formatDuration(a.elapsed), label: a.phase === "running" ? "elapsed" : a.phase.replace("_", "-"), progress: null };
  const remaining = Math.max(0, a.duration - a.elapsed);
  const progress = a.duration > 0 ? Math.min(1, Math.max(0, a.elapsed / a.duration)) : 1;
  const label = a.phase === "pre_wait" ? "pre-wait" : a.phase === "post_wait" ? "post-wait" : "left";
  return { text: `-${remaining >= 60 ? formatDuration(remaining, false) : formatDuration(remaining)}`, label, progress };
}

interface RowProps {
  cue: Cue;
  depth: number;
  show: Show;
  active: ActiveCue | undefined;
  error: string | undefined;
  loaded: number | undefined;
  isPlayhead: boolean;
  selected: boolean;
  interactive: boolean;
  gutterSetsPlayhead: boolean;
  onSelect: (id: number) => void;
  onPlayhead: (id: number) => void;
}

const LiveRow = memo(function LiveRow(p: RowProps) {
  const { cue, active } = p;
  const st = rowState(active, p.error, p.loaded);
  const timing = active ? activeTiming(active) : null;
  const dur = cueDuration(cue);
  const cont = CONTINUE_GLYPH[cue.continue_mode];
  const cls = [
    "lrow",
    `is-${st}`,
    p.isPlayhead ? "is-playhead" : "",
    p.selected ? "is-selected" : "",
    cue.armed ? "" : "is-disarmed",
    cue.kind.type === "group" ? "is-group" : "",
    cue.color ? `c-${cue.color}` : "",
  ].join(" ");
  return (
    <div
      className={cls}
      data-cue={cue.id}
      data-state={st}
      data-playhead={p.isPlayhead ? "1" : undefined}
      role="row"
      aria-selected={p.selected}
      onClick={p.interactive ? () => p.onSelect(cue.id) : undefined}
    >
      {p.gutterSetsPlayhead ? (
        <button
          className="lrow-gutter"
          title="Set playhead (NEXT) here"
          aria-label={`Set playhead to cue ${cue.number}`}
          onClick={(e) => {
            e.stopPropagation();
            p.onPlayhead(cue.id);
          }}
        >
          {p.isPlayhead ? <span className="tag tag-next">NEXT</span> : null}
        </button>
      ) : (
        <span className="lrow-gutter">{p.isPlayhead ? <span className="tag tag-next">NEXT</span> : null}</span>
      )}
      <span className="lrow-status"><StatusIcon state={st} armed={cue.armed} size={18} /></span>
      <span className="lrow-num num" style={p.depth ? { paddingLeft: p.depth * 14 } : undefined}>{cue.number}</span>
      <span className="lrow-type" title={TYPE_LABELS[cue.kind.type]}><TypeIcon type={cue.kind.type} size={18} /></span>
      <span className="lrow-main">
        <span className="lrow-name">{cue.name || <em className="muted">{TYPE_LABELS[cue.kind.type]}</em>}</span>
        {p.error ? <span className="lrow-sub lrow-err">{p.error}</span> : cue.kind.type !== "memo" && cue.kind.type !== "audio" ? <span className="lrow-sub">{cueTarget(cue, p.show)}</span> : cue.notes ? <span className="lrow-sub">{cue.notes}</span> : null}
      </span>
      <span className="lrow-tags">
        {!cue.armed ? <span className="tag tag-disarmed">DISARMED</span> : null}
        {st === "paused" ? <span className="tag tag-paused">PAUSED</span> : null}
        {st === "loaded" ? <span className="tag tag-loaded">LOADED {formatDuration(p.loaded ?? 0, false)}</span> : null}
        {cue.tc_trigger ? <span className="tag tag-tc num">TC {cue.tc_trigger}</span> : null}
        {st === "pre_wait" ? <span className="tag tag-wait">PRE-WAIT</span> : null}
        {st === "post_wait" ? <span className="tag tag-wait">POST-WAIT</span> : null}
      </span>
      <span className="lrow-time num">
        {timing ? (
          <>
            <b>{st === "pre_wait" ? formatCountdown(Math.max(0, (active?.duration ?? 0) - (active?.elapsed ?? 0))) : timing.text}</b>
            <small>{timing.label}</small>
          </>
        ) : dur != null ? (
          formatDuration(dur, false)
        ) : cue.kind.type === "audio" && cue.kind.loop_count === 0 ? (
          "∞"
        ) : (
          ""
        )}
      </span>
      <span className="lrow-cont" title={cont.title}>{cont.glyph}</span>
      {timing ? (
        <span className="lrow-progress" aria-hidden="true">
          <span className={timing.progress == null ? "indeterminate" : ""} style={timing.progress != null ? { width: `${(timing.progress * 100).toFixed(2)}%` } : undefined} />
        </span>
      ) : null}
    </div>
  );
});

const NO_ACTIVE: ActiveCue[] = [];

export function CueList(p: {
  variant: "desktop" | "touch" | "view";
  /** rows can be selected (tap/click) */
  selectable: boolean;
  /** gutter click sets the playhead (sends a command) */
  gutterSetsPlayhead?: boolean;
  follow: boolean;
  onPlayhead?: (id: number) => void;
}) {
  const show = useApp((s) => s.doc?.show ?? null);
  const listId = useApp((s) => currentListId(s));
  const playhead = useApp((s) => (listId != null ? s.doc?.playheads[String(listId)] ?? null : null));
  const errors = useApp((s) => s.doc?.errors);
  const loaded = useApp((s) => s.doc?.loaded);
  const active = useApp((s) => s.tick?.active ?? NO_ACTIVE);
  const selection = useApp((s) => s.selection);
  const list = findList(show, listId);
  const rows = useMemo(() => (list ? flattenCues(list.cues) : []), [list]);
  const amap = useMemo(() => activeMap(active), [active]);
  const ref = useRef<HTMLDivElement>(null);

  // Keep the NEXT row in view (also after the list box changes size, e.g. device rotation).
  useEffect(() => {
    const box = ref.current;
    if (!p.follow || playhead == null || !box) return;
    const reveal = () => {
      const row = box.querySelector<HTMLElement>(`[data-cue="${playhead}"]`);
      if (!row) return;
      const visibleTop = box.scrollTop;
      const visibleBottom = visibleTop + box.clientHeight;
      const margin = Math.min(80, Math.max(0, (box.clientHeight - row.offsetHeight) / 2));
      if (row.offsetTop >= visibleTop && row.offsetTop + row.offsetHeight <= visibleBottom - margin) return;
      // Scroll so that the NEXT row sits about a third down, aligned to a row edge (no half-cut row on top).
      const desired = row.offsetTop - box.clientHeight * 0.3;
      let top = 0;
      for (const el of Array.from(box.children) as HTMLElement[]) {
        if (el.offsetTop <= desired) top = el.offsetTop;
        else break;
      }
      // Tiny list boxes (small phones): the NEXT row itself must always be fully visible.
      if (row.offsetTop + row.offsetHeight > top + box.clientHeight) top = row.offsetTop;
      box.scrollTo({ top: Math.max(0, top), behavior: "auto" });
    };
    reveal();
    if (typeof ResizeObserver === "undefined") return;
    const ro = new ResizeObserver(reveal);
    ro.observe(box);
    return () => ro.disconnect();
  }, [playhead, p.follow, rows.length]);

  const onSelect = useMemo(() => (id: number) => setApp({ selection: [id] }), []);
  const onPlayhead = p.onPlayhead ?? noop;

  if (!show || !list) return <div className="cuelist empty">No cue list</div>;
  return (
    <div className={`cuelist cuelist-${p.variant}`} ref={ref} role="grid" aria-label={`Cue list ${list.name}`} data-testid="cuelist">
      {rows.length === 0 ? <div className="cuelist-empty">This cue list is empty.</div> : null}
      {rows.map((r) => (
        <LiveRow
          key={r.cue.id}
          cue={r.cue}
          depth={r.depth}
          show={show}
          active={amap.get(r.cue.id)}
          error={errors?.[String(r.cue.id)]}
          loaded={loaded?.[String(r.cue.id)]}
          isPlayhead={playhead === r.cue.id}
          selected={p.selectable && selection.includes(r.cue.id)}
          interactive={p.selectable}
          gutterSetsPlayhead={!!p.gutterSetsPlayhead}
          onSelect={onSelect}
          onPlayhead={onPlayhead}
        />
      ))}
      {playhead == null && rows.length > 0 ? <div className="cuelist-end">End of list – no cue standing by</div> : null}
    </div>
  );
}

function noop(): void {}
