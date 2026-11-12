// OSC argument text <-> JSON values (mirrors sm-core/src/osc.rs).
// Note: JSON from the browser cannot tell 1.0 from 1, so whole-number floats
// typed here are sent as ints; use the desktop app for those.

export type OscArg = number | boolean | string;

export function parseOscArgs(text: string): OscArg[] {
  const out: OscArg[] = [];
  let i = 0;
  while (i < text.length) {
    while (i < text.length && /\s/.test(text[i])) i++;
    if (i >= text.length) break;
    if (text[i] === '"') {
      i++;
      let s = "";
      while (i < text.length && text[i] !== '"') {
        if (text[i] === "\\" && i + 1 < text.length) i++;
        s += text[i++];
      }
      i++;
      out.push(s);
      continue;
    }
    let w = "";
    while (i < text.length && !/\s/.test(text[i])) w += text[i++];
    if (w === "true") out.push(true);
    else if (w === "false") out.push(false);
    else if (/^[+-]?\d+$/.test(w) && Math.abs(Number(w)) <= 2147483647) out.push(Number(w));
    else if (/^[+-]?(\d+\.?\d*|\.\d+)([eE][+-]?\d+)?$/.test(w)) out.push(Number(w));
    else out.push(w);
  }
  return out;
}

export function formatOscArgs(args: OscArg[]): string {
  return args
    .map((a) => {
      if (typeof a !== "string") return String(a);
      const plain = a !== "" && !/[\s"\\]/.test(a) && parseOscArgs(a)[0] === a;
      return plain ? a : `"${a.replace(/\\/g, "\\\\").replace(/"/g, '\\"')}"`;
    })
    .join(" ");
}

export function validateOscAddress(a: string): string | null {
  if (!a.startsWith("/")) return "The address must start with /";
  if (/[\s#]/.test(a)) return "No spaces or # in the address";
  return null;
}
