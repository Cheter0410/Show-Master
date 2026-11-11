import type { PatchOp, PathSeg } from "./api";

type Container = Record<string, unknown> | unknown[];

function isContainer(x: unknown): x is Container {
  return typeof x === "object" && x !== null;
}

function cloneContainer(x: unknown, nextSeg: PathSeg): Container {
  if (Array.isArray(x)) return x.slice();
  if (isContainer(x)) return { ...x };
  // Missing intermediate: create the container the next segment implies.
  return typeof nextSeg === "number" ? [] : {};
}

function setIn(node: unknown, path: PathSeg[], i: number, op: PatchOp): unknown {
  if (i === path.length) return op.d ? undefined : op.v;
  const seg = path[i];
  const last = i === path.length - 1;
  if (last && op.d) {
    if (!isContainer(node)) return node; // nothing to delete
    if (Array.isArray(node)) {
      const idx = Number(seg);
      if (!Number.isInteger(idx) || idx < 0 || idx >= node.length) return node;
      const copy = node.slice();
      copy.splice(idx, 1);
      return copy;
    }
    const key = String(seg);
    if (!(key in node)) return node;
    const copy = { ...node };
    delete copy[key];
    return copy;
  }
  if (op.d && !isContainer(node)) return node; // deleting below something that does not exist
  const copy = cloneContainer(node, seg);
  if (Array.isArray(copy)) {
    const idx = Number(seg);
    const next = setIn(copy[idx], path, i + 1, op);
    if (op.d && next === copy[idx]) return node;
    copy[idx] = next;
  } else {
    const key = String(seg);
    const next = setIn(copy[key], path, i + 1, op);
    if (op.d && next === copy[key]) return node;
    copy[key] = next;
  }
  return copy;
}

/**
 * Applies WebSocket `patch.ops` to the state document without mutating it.
 * Only the objects along each path are copied (structural sharing), so React
 * selectors on untouched sub-trees keep their identity.
 */
export function applyPatch<T>(doc: T, ops: readonly PatchOp[]): T {
  let out: unknown = doc;
  for (const op of ops) {
    if (!Array.isArray(op.p)) continue;
    if (op.p.length === 0) {
      if (!op.d) out = op.v;
      continue;
    }
    out = setIn(out, op.p, 0, op);
  }
  return out as T;
}
