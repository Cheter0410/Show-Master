// Pop-up menus: toolbar drop-downs and the cue row context menu (right-click or long-press).
// On phones the menu is a bottom sheet with large rows.
import { useEffect, useLayoutEffect, useRef, useState, type ReactNode } from "react";
import { PHONE, useMedia } from "../platform";
import { IconChevronDown } from "./icons";

export interface MenuItem {
  label: ReactNode;
  icon?: ReactNode;
  onSelect: () => void;
  disabled?: boolean;
  danger?: boolean;
  testId?: string;
  /** draw a separator above this item */
  separator?: boolean;
}

export interface MenuAnchor {
  x: number;
  y: number;
  /** extra space to keep free above `y` when flipping upwards (anchor height) */
  flipBy?: number;
}

/** Menu at a point (clamped to the viewport). Marked `data-modal` so hotkeys stay away while open. */
export function Menu(p: { at: MenuAnchor; items: MenuItem[]; onClose: () => void; heading?: ReactNode; testId?: string }) {
  const sheet = useMedia(PHONE);
  const ref = useRef<HTMLDivElement>(null);
  const opened = useRef(performance.now());
  // Pointer clicks count only after a fresh press inside the menu: the release that ends a long-press
  // (or a touch browser's synthetic click) must not pick the item that appears under the finger.
  const armed = useRef(false);
  const [pos, setPos] = useState<{ left: number; top: number } | null>(null);
  const onClose = useRef(p.onClose);
  onClose.current = p.onClose;

  useLayoutEffect(() => {
    const el = ref.current;
    if (!el || sheet) return;
    const r = el.getBoundingClientRect();
    const W = window.innerWidth;
    const H = window.innerHeight;
    let left = Math.min(p.at.x, W - r.width - 8);
    let top = p.at.y;
    if (top + r.height > H - 8) top = Math.max(8, p.at.y - (p.at.flipBy ?? 0) - r.height);
    left = Math.max(8, left);
    setPos({ left, top });
  }, [p.at.x, p.at.y, p.at.flipBy, sheet]);

  useEffect(() => {
    const prev = document.activeElement as HTMLElement | null;
    ref.current?.querySelector<HTMLElement>("[role=menuitem]:not(:disabled)")?.focus({ preventScroll: true });
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.preventDefault();
        e.stopPropagation();
        onClose.current();
      }
    };
    const onResize = () => onClose.current();
    window.addEventListener("keydown", onKey, true);
    window.addEventListener("resize", onResize);
    return () => {
      window.removeEventListener("keydown", onKey, true);
      window.removeEventListener("resize", onResize);
      prev?.focus?.({ preventScroll: true });
    };
  }, []);

  const move = (dir: 1 | -1) => {
    const items = Array.from(ref.current?.querySelectorAll<HTMLElement>("[role=menuitem]:not(:disabled)") ?? []);
    if (items.length === 0) return;
    const i = items.indexOf(document.activeElement as HTMLElement);
    items[(i + dir + items.length) % items.length].focus();
  };

  return (
    <div
      className={`menu-layer${sheet ? " sheet" : ""}`}
      data-modal="1"
      onPointerDownCapture={() => {
        armed.current = true;
      }}
      onClick={(e) => {
        if (e.target === e.currentTarget && armed.current) p.onClose();
      }}
      onContextMenu={(e) => {
        e.preventDefault();
        // a touch browser's own long-press `contextmenu` can land on the layer right after it opened
        if (e.target === e.currentTarget && performance.now() - opened.current > 1000) p.onClose();
      }}
    >
      <div
        ref={ref}
        className="menu"
        role="menu"
        data-testid={p.testId ?? "menu"}
        style={sheet ? undefined : pos ? { left: pos.left, top: pos.top } : { left: p.at.x, top: p.at.y, visibility: "hidden" }}
        onKeyDown={(e) => {
          if (e.key === "ArrowDown") {
            e.preventDefault();
            move(1);
          } else if (e.key === "ArrowUp") {
            e.preventDefault();
            move(-1);
          }
        }}
      >
        {p.heading ? <div className="menu-heading ellipsis">{p.heading}</div> : null}
        {p.items.map((it, i) => (
          <button
            key={i}
            type="button"
            role="menuitem"
            className={`menu-item${it.danger ? " danger" : ""}${it.separator ? " sep" : ""}`}
            disabled={it.disabled}
            data-testid={it.testId}
            onClick={(e) => {
              if (e.detail !== 0 && !armed.current) return; // keyboard activation has detail 0
              p.onClose();
              it.onSelect();
            }}
          >
            <span className="menu-icon">{it.icon}</span>
            <span className="menu-label">{it.label}</span>
          </button>
        ))}
        {sheet ? (
          <button type="button" className="menu-item menu-cancel" onClick={(e) => (e.detail === 0 || armed.current) && p.onClose()}>
            Cancel
          </button>
        ) : null}
      </div>
    </div>
  );
}

/** Button that opens a drop-down menu below itself. */
export function MenuButton(p: { label: ReactNode; items: MenuItem[]; className?: string; testId?: string; ariaLabel?: string; heading?: ReactNode; caret?: boolean }) {
  const [at, setAt] = useState<MenuAnchor | null>(null);
  const ref = useRef<HTMLButtonElement>(null);
  return (
    <>
      <button
        ref={ref}
        type="button"
        className={`btn${p.className ? ` ${p.className}` : ""}`}
        aria-haspopup="menu"
        aria-expanded={at != null}
        aria-label={p.ariaLabel}
        data-testid={p.testId}
        onClick={() => {
          if (at) return setAt(null);
          const r = ref.current?.getBoundingClientRect();
          if (r) setAt({ x: r.left, y: r.bottom + 4, flipBy: r.height + 8 });
        }}
      >
        {p.label}
        {p.caret !== false ? <IconChevronDown size={12} /> : null}
      </button>
      {at ? <Menu at={at} items={p.items} heading={p.heading} onClose={() => setAt(null)} testId={p.testId ? `${p.testId}-list` : undefined} /> : null}
    </>
  );
}

const LONG_PRESS_MS = 550;

/**
 * Long-press for touch / pen (mouse users get the native `contextmenu` event). Returns pointer handlers
 * for the element; `onLongPress` gets the press position.
 */
export function useLongPress(onLongPress: (x: number, y: number) => void) {
  const st = useRef<{ timer: ReturnType<typeof setTimeout>; x: number; y: number } | null>(null);
  const cb = useRef(onLongPress);
  cb.current = onLongPress;
  const cancel = () => {
    if (st.current) clearTimeout(st.current.timer);
    st.current = null;
  };
  useEffect(() => cancel, []);
  return {
    onPointerDown: (e: React.PointerEvent) => {
      if (e.pointerType === "mouse") return;
      cancel();
      const x = e.clientX;
      const y = e.clientY;
      st.current = {
        x,
        y,
        timer: setTimeout(() => {
          st.current = null;
          navigator.vibrate?.(15);
          cb.current(x, y);
        }, LONG_PRESS_MS),
      };
    },
    onPointerMove: (e: React.PointerEvent) => {
      if (st.current && Math.hypot(e.clientX - st.current.x, e.clientY - st.current.y) > 10) cancel();
    },
    onPointerUp: cancel,
    onPointerCancel: cancel,
  };
}
