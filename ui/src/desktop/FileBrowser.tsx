// Modal file browser backed by GET /api/fs (the browser has no access to real paths).
import { useEffect, useState } from "react";
import { fetchFs, HttpError, type FsEntry, type FsListing } from "../api";
import { Modal } from "../components/Modal";
import { IconFile, IconFolder, IconUp } from "../components/icons";
import { isAudioPath } from "../platform";
import { getApp } from "../store";

export type FileMode = "open-show" | "save-show" | "pick-audio";

const TITLES: Record<FileMode, string> = { "open-show": "Open show", "save-show": "Save show as", "pick-audio": "Choose audio file" };

function formatSize(n: number): string {
  if (n >= 1 << 30) return `${(n / (1 << 30)).toFixed(1)} GB`;
  if (n >= 1 << 20) return `${(n / (1 << 20)).toFixed(1)} MB`;
  if (n >= 1024) return `${Math.round(n / 1024)} kB`;
  return `${n} B`;
}

let lastDir: Record<FileMode, string> = { "open-show": "", "save-show": "", "pick-audio": "" };

export function FileBrowser(p: { mode: FileMode; initialPath?: string; multiple?: boolean; onPick: (paths: string[]) => void; onClose: () => void }) {
  const [listing, setListing] = useState<FsListing | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [sel, setSel] = useState<string[]>([]);
  const [name, setName] = useState(() => (p.mode === "save-show" ? (getApp().doc?.show.name ?? "show").replace(/[\\/:*?"<>|]/g, "_") + ".smshow" : ""));

  const load = (path: string) => {
    const token = getApp().token;
    if (!token) return;
    setLoading(true);
    setError(null);
    fetchFs(token, path).then(
      (l) => {
        setListing(l);
        setSel([]);
        setLoading(false);
        lastDir = { ...lastDir, [p.mode]: l.path };
      },
      (e: unknown) => {
        setLoading(false);
        setError(e instanceof HttpError && e.status === 403 ? "Not permitted for this role" : e instanceof HttpError && e.status === 404 ? "Folder not found" : "Could not read the folder");
        if (path !== "" && !listing) load("");
      },
    );
  };
  useEffect(() => {
    load(p.initialPath || lastDir[p.mode] || "");
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const wanted = (e: FsEntry) => e.dir || (p.mode === "pick-audio" ? isAudioPath(e.name) : /\.smshow$/i.test(e.name));
  const entries = listing?.entries.filter(wanted) ?? [];
  const confirm = () => {
    if (p.mode === "save-show") {
      const n = name.trim();
      if (!n || !listing) return;
      const file = /\.smshow$/i.test(n) ? n : `${n}.smshow`;
      p.onPick([`${listing.path.replace(/\/$/, "")}/${file}`]);
    } else if (sel.length > 0) p.onPick(sel);
  };
  const canConfirm = p.mode === "save-show" ? name.trim() !== "" && listing != null : sel.length > 0;
  const overwrites = p.mode === "save-show" && entries.some((e) => !e.dir && (e.name === name.trim() || e.name === `${name.trim()}.smshow`));

  return (
    <Modal
      title={TITLES[p.mode]}
      onClose={p.onClose}
      wide
      testId="file-browser"
      footer={
        <>
          {p.mode === "save-show" ? (
            <input className="input fb-name" value={name} aria-label="File name" data-testid="fb-name" onChange={(e) => setName(e.target.value)} onKeyDown={(e) => e.key === "Enter" && confirm()} />
          ) : (
            <span className="muted fb-count">{sel.length > 1 ? `${sel.length} files selected` : ""}</span>
          )}
          <button className="btn" onClick={p.onClose}>
            Cancel
          </button>
          <button className="btn primary" disabled={!canConfirm} onClick={confirm} data-testid="fb-ok">
            {p.mode === "save-show" ? (overwrites ? "Overwrite" : "Save") : p.mode === "open-show" ? "Open" : "Add"}
          </button>
        </>
      }
    >
      <div className="fb-path">
        <button className="btn icon-only" disabled={!listing?.parent} onClick={() => listing?.parent != null && load(listing.parent)} aria-label="Parent folder" title="Parent folder">
          <IconUp />
        </button>
        <span className="mono ellipsis" data-testid="fb-path">{listing?.path ?? "…"}</span>
        {loading ? <span className="muted">loading…</span> : null}
      </div>
      {error ? <div className="fb-error" role="alert">{error}</div> : null}
      <div className="fb-list" role="listbox" aria-multiselectable={p.multiple}>
        {entries.length === 0 && !loading ? <div className="muted fb-empty">No matching files in this folder</div> : null}
        {entries.map((e) => (
          <div
            key={e.path}
            role="option"
            aria-selected={sel.includes(e.path)}
            className={`fb-entry${sel.includes(e.path) ? " sel" : ""}`}
            data-testid="fb-entry"
            data-name={e.name}
            onClick={(ev) => {
              if (e.dir) return;
              if (p.mode === "save-show") setName(e.name);
              else if (p.multiple && (ev.ctrlKey || ev.metaKey || ev.shiftKey)) setSel((s) => (s.includes(e.path) ? s.filter((x) => x !== e.path) : [...s, e.path]));
              else setSel([e.path]);
            }}
            onDoubleClick={() => {
              if (e.dir) load(e.path);
              else if (p.mode !== "save-show") p.onPick([e.path]);
            }}
          >
            {e.dir ? <IconFolder /> : <IconFile />}
            <span className="ellipsis fb-entry-name">{e.name}</span>
            {e.dir ? (
              <button className="btn small" onClick={() => load(e.path)}>
                Open
              </button>
            ) : (
              <span className="muted num">{formatSize(e.size)}</span>
            )}
          </div>
        ))}
      </div>
    </Modal>
  );
}
