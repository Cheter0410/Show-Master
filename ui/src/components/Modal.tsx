import { useEffect, useRef, type ReactNode } from "react";
import { IconX } from "./icons";

/** Simple modal dialog. Escape closes it (and never reaches the panic hotkey). */
export function Modal(p: { title: ReactNode; onClose?: () => void; children: ReactNode; footer?: ReactNode; wide?: boolean; className?: string; testId?: string }) {
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const prev = document.activeElement as HTMLElement | null;
    const el = ref.current;
    if (el && !el.contains(document.activeElement)) {
      const auto = el.querySelector<HTMLElement>("[data-autofocus]");
      (auto ?? el).focus();
    }
    return () => prev?.focus?.();
  }, []);
  return (
    <div
      className="modal-backdrop"
      data-modal="1"
      onPointerDown={(e) => {
        if (e.target === e.currentTarget) p.onClose?.();
      }}
    >
      <div
        ref={ref}
        className={`modal${p.wide ? " wide" : ""}${p.className ? ` ${p.className}` : ""}`}
        role="dialog"
        aria-modal="true"
        tabIndex={-1}
        data-testid={p.testId}
        onKeyDown={(e) => {
          if (e.key === "Escape" && p.onClose) {
            e.stopPropagation();
            p.onClose();
          }
        }}
      >
        <header className="modal-head">
          <h2>{p.title}</h2>
          {p.onClose ? (
            <button className="btn ghost icon-only" onClick={p.onClose} aria-label="Close">
              <IconX />
            </button>
          ) : null}
        </header>
        <div className="modal-body">{p.children}</div>
        {p.footer ? <footer className="modal-foot">{p.footer}</footer> : null}
      </div>
    </div>
  );
}

export function Confirm(p: { title: string; children: ReactNode; confirmLabel: string; danger?: boolean; onConfirm: () => void; onCancel: () => void; cancelLabel?: string }) {
  return (
    <Modal
      title={p.title}
      onClose={p.onCancel}
      testId="confirm"
      footer={
        <>
          <button className="btn" onClick={p.onCancel} data-autofocus>
            {p.cancelLabel ?? "Cancel"}
          </button>
          <button className={`btn ${p.danger ? "danger" : "primary"}`} onClick={p.onConfirm} data-testid="confirm-ok">
            {p.confirmLabel}
          </button>
        </>
      }
    >
      <div className="confirm-text">{p.children}</div>
    </Modal>
  );
}
