import { cuesAfter, cueTarget, findCue, findList, TYPE_LABELS } from "../cues";
import { currentListId, useApp } from "../store";
import { TypeIcon } from "./icons";

/** The cue standing by at the playhead (huge) plus the cues after it. */
export function NextPanel({ following = 3, compact = false }: { following?: number; compact?: boolean }) {
  const show = useApp((s) => s.doc?.show ?? null);
  const listId = useApp((s) => currentListId(s));
  const playhead = useApp((s) => (listId != null ? s.doc?.playheads[String(listId)] ?? null : null));
  const error = useApp((s) => (playhead != null ? s.doc?.errors[String(playhead)] : undefined));
  const cue = findCue(show, playhead);
  const after = cuesAfter(findList(show, listId), playhead, following);
  return (
    <section className={`next-panel${compact ? " compact" : ""}${cue ? "" : " is-empty"}${error ? " has-error" : ""}`} data-testid="next-panel" aria-label="Next cue">
      <div className="next-head">
        <span className="tag tag-next">NEXT</span>
        {cue && !cue.armed ? <span className="tag tag-disarmed">DISARMED</span> : null}
        {error ? <span className="tag tag-error">BROKEN</span> : null}
      </div>
      {cue && show ? (
        <>
          <div className="next-cue">
            <span className="next-num num" data-testid="next-number">{cue.number || "–"}</span>
            <span className="next-name">{cue.name || TYPE_LABELS[cue.kind.type]}</span>
          </div>
          <div className="next-meta">
            <TypeIcon type={cue.kind.type} /> <span>{TYPE_LABELS[cue.kind.type]}</span>
            {cue.kind.type !== "memo" ? <span className="muted ellipsis">· {cueTarget(cue, show)}</span> : null}
          </div>
          {error ? <div className="next-error">{error}</div> : null}
          {cue.notes ? <div className="next-notes">{cue.notes}</div> : null}
        </>
      ) : (
        <div className="next-cue">
          <span className="next-name muted">Nothing standing by</span>
        </div>
      )}
      {after.length > 0 ? (
        <ol className="next-after" aria-label="Following cues">
          {after.map((c) => (
            <li key={c.id} className={c.armed ? "" : "is-disarmed"}>
              <span className="num">{c.number}</span>
              <TypeIcon type={c.kind.type} size={14} />
              <span className="ellipsis">{c.name || TYPE_LABELS[c.kind.type]}</span>
            </li>
          ))}
        </ol>
      ) : null}
    </section>
  );
}
