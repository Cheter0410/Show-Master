// Form fields that commit on blur / Enter (Escape reverts). Server state stays the source of truth:
// while a field is not being edited it always shows the current value from the store.
import { useEffect, useId, useRef, useState, type ReactNode } from "react";
import { formatDb, formatSeconds, normalizeTimecode, parseDb, parseDuration, validateTimecode } from "../time";

export type Parsed<T> = { ok: true; value: T } | { ok: false; error: string };

interface DraftFieldProps<T> {
  value: T;
  format: (v: T) => string;
  parse: (text: string) => Parsed<T>;
  onCommit: (v: T) => void;
  equals?: (a: T, b: T) => boolean;
  disabled?: boolean;
  placeholder?: string;
  className?: string;
  inputMode?: "text" | "decimal" | "numeric";
  width?: number | string;
  ariaLabel?: string;
  testId?: string;
  autoFocus?: boolean;
  onDone?: () => void;
  mono?: boolean;
}

export function DraftField<T>(p: DraftFieldProps<T>) {
  const [draft, setDraft] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const skip = useRef(false);
  const shown = draft ?? p.format(p.value);

  const commit = () => {
    if (skip.current) {
      skip.current = false;
      setDraft(null);
      setError(null);
      p.onDone?.();
      return;
    }
    if (draft == null) {
      p.onDone?.();
      return;
    }
    const r = p.parse(draft);
    if (!r.ok) {
      setError(r.error);
      return; // keep the draft visible so the user can fix it
    }
    const same = p.equals ? p.equals(r.value, p.value) : r.value === p.value;
    if (!same) p.onCommit(r.value);
    setDraft(null);
    setError(null);
    p.onDone?.();
  };

  return (
    <span className={`field-wrap${error ? " has-error" : ""}`} style={p.width != null ? { width: p.width } : undefined}>
      <input
        className={`input${p.mono ? " mono" : ""}${p.className ? ` ${p.className}` : ""}`}
        type="text"
        value={shown}
        disabled={p.disabled}
        placeholder={p.placeholder}
        inputMode={p.inputMode}
        aria-label={p.ariaLabel}
        aria-invalid={error ? true : undefined}
        data-testid={p.testId}
        autoFocus={p.autoFocus}
        autoComplete="off"
        autoCorrect="off"
        spellCheck={false}
        onFocus={(e) => {
          if (p.autoFocus) e.currentTarget.select();
        }}
        onChange={(e) => {
          setDraft(e.target.value);
          if (error) setError(null);
        }}
        onBlur={commit}
        onKeyDown={(e) => {
          if (e.key === "Enter") {
            e.preventDefault();
            e.currentTarget.blur();
          } else if (e.key === "Escape") {
            e.stopPropagation();
            skip.current = true;
            e.currentTarget.blur();
          }
        }}
      />
      {error ? <span className="field-error" role="alert">{error}</span> : null}
    </span>
  );
}

type Common = Pick<DraftFieldProps<unknown>, "disabled" | "placeholder" | "width" | "ariaLabel" | "testId" | "autoFocus" | "onDone" | "className">;

export function TextField(p: Common & { value: string; onCommit: (v: string) => void }) {
  return <DraftField<string> {...p} format={(v) => v} parse={(t) => ({ ok: true, value: t })} />;
}

export function NumField(
  p: Common & { value: number; onCommit: (v: number) => void; min?: number; max?: number; integer?: boolean },
) {
  return (
    <DraftField<number>
      {...p}
      mono
      inputMode={p.integer ? "numeric" : "decimal"}
      format={(v) => String(v)}
      parse={(t) => {
        const s = t.trim().replace(",", ".");
        if (!/^[+-]?(\d+(\.\d*)?|\.\d+)$/.test(s)) return { ok: false, error: "Not a number" };
        const n = Number(s);
        if (p.integer && !Number.isInteger(n)) return { ok: false, error: "Whole number required" };
        if (p.min != null && n < p.min) return { ok: false, error: `Minimum is ${p.min}` };
        if (p.max != null && n > p.max) return { ok: false, error: `Maximum is ${p.max}` };
        return { ok: true, value: n };
      }}
    />
  );
}

/** Seconds; accepts "12.5", "1:30", "1:02:03.5". With `nullable`, empty text means null. */
export function TimeField(p: Common & { value: number | null; onCommit: (v: number | null) => void; nullable?: boolean; max?: number }) {
  return (
    <DraftField<number | null>
      {...p}
      mono
      inputMode="text"
      format={(v) => (v == null ? "" : v === 0 ? "0" : formatSeconds(v))}
      parse={(t) => {
        if (t.trim() === "") return p.nullable ? { ok: true, value: null } : { ok: true, value: 0 };
        const n = parseDuration(t);
        if (n == null) return { ok: false, error: "Use seconds or m:ss.s" };
        if (p.max != null && n > p.max) return { ok: false, error: `Maximum is ${formatSeconds(p.max)}` };
        return { ok: true, value: n };
      }}
    />
  );
}

export function DbField(p: Common & { value: number | null; onCommit: (v: number | null) => void; nullable?: boolean; min?: number; max?: number }) {
  return (
    <DraftField<number | null>
      {...p}
      mono
      format={(v) => (v == null ? "" : formatDb(v))}
      parse={(t) => {
        if (t.trim() === "" && p.nullable) return { ok: true, value: null };
        const n = parseDb(t);
        if (n == null) return { ok: false, error: "dB value, e.g. -6 or -inf" };
        return { ok: true, value: Math.min(p.max ?? 24, Math.max(p.min ?? -120, n)) };
      }}
    />
  );
}

/** HH:MM:SS:FF validated against the show's frame rate. With `nullable`, empty = null. */
export function TcField(p: Common & { value: string | null; rate: string; onCommit: (v: string | null) => void; nullable?: boolean }) {
  return (
    <DraftField<string | null>
      {...p}
      mono
      placeholder={p.placeholder ?? "HH:MM:SS:FF"}
      format={(v) => v ?? ""}
      parse={(t) => {
        if (t.trim() === "") return p.nullable ? { ok: true, value: null } : { ok: false, error: "Timecode required" };
        const norm = normalizeTimecode(t);
        if (!norm) return { ok: false, error: "Format must be HH:MM:SS:FF" };
        const err = validateTimecode(norm, p.rate);
        return err ? { ok: false, error: err } : { ok: true, value: norm };
      }}
    />
  );
}

export function Select<T extends string | number>(p: {
  value: T;
  options: ReadonlyArray<{ value: T; label: string }>;
  onChange: (v: T) => void;
  disabled?: boolean;
  ariaLabel?: string;
  testId?: string;
  width?: number | string;
}) {
  const idx = p.options.findIndex((o) => o.value === p.value);
  return (
    <select
      className="input select"
      style={p.width != null ? { width: p.width } : undefined}
      value={idx}
      disabled={p.disabled}
      aria-label={p.ariaLabel}
      data-testid={p.testId}
      onChange={(e) => {
        const o = p.options[Number(e.target.value)];
        if (o) p.onChange(o.value);
      }}
    >
      {idx < 0 ? <option value={-1}>{String(p.value ?? "") || "—"}</option> : null}
      {p.options.map((o, i) => (
        <option key={i} value={i}>
          {o.label}
        </option>
      ))}
    </select>
  );
}

export function Check(p: { checked: boolean; onChange: (v: boolean) => void; label: ReactNode; disabled?: boolean; testId?: string; indeterminate?: boolean }) {
  const ref = useRef<HTMLInputElement>(null);
  useEffect(() => {
    if (ref.current) ref.current.indeterminate = !!p.indeterminate;
  }, [p.indeterminate]);
  return (
    <label className={`check${p.disabled ? " disabled" : ""}`}>
      <input ref={ref} type="checkbox" checked={p.checked} disabled={p.disabled} data-testid={p.testId} onChange={(e) => p.onChange(e.target.checked)} />
      <span>{p.label}</span>
    </label>
  );
}

/** Range slider: shows the dragged value immediately, sends debounced while dragging and once on release. */
export function Slider(p: { value: number; min: number; max: number; step: number; onChange: (v: number) => void; disabled?: boolean; ariaLabel?: string; debounceMs?: number; onDoubleClick?: () => void }) {
  const [drag, setDrag] = useState<number | null>(null);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const latest = useRef(p.onChange);
  latest.current = p.onChange;
  const flush = (v: number) => {
    if (timer.current) clearTimeout(timer.current);
    timer.current = null;
    latest.current(v);
  };
  useEffect(() => () => {
    if (timer.current) clearTimeout(timer.current);
  }, []);
  return (
    <input
      className="slider"
      type="range"
      min={p.min}
      max={p.max}
      step={p.step}
      value={drag ?? Math.min(p.max, Math.max(p.min, p.value))}
      disabled={p.disabled}
      aria-label={p.ariaLabel}
      onChange={(e) => {
        const v = Number(e.target.value);
        setDrag(v);
        if (timer.current) clearTimeout(timer.current);
        timer.current = setTimeout(() => flush(v), p.debounceMs ?? 120);
      }}
      onPointerUp={() => {
        if (drag != null) flush(drag);
        setTimeout(() => setDrag(null), 300);
      }}
      onKeyUp={() => {
        if (drag != null) flush(drag);
        setTimeout(() => setDrag(null), 300);
      }}
      onBlur={() => setDrag(null)}
      onDoubleClick={p.onDoubleClick}
    />
  );
}

export function Row({ label, children, hint }: { label: ReactNode; children: ReactNode; hint?: ReactNode }) {
  const id = useId();
  return (
    <div className="form-row" role="group" aria-labelledby={id}>
      <span className="form-label" id={id}>{label}</span>
      <div className="form-ctl">
        {children}
        {hint ? <span className="hint">{hint}</span> : null}
      </div>
    </div>
  );
}
