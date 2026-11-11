// Inline SVG icons (no icon font, no external assets).
import type { ReactNode } from "react";
import type { CueType } from "../api";

function Svg({ children, size = 16, title }: { children: ReactNode; size?: number; title?: string }) {
  return (
    <svg
      className="icon"
      width={size}
      height={size}
      viewBox="0 0 16 16"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.6"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden={title ? undefined : true}
      role={title ? "img" : undefined}
    >
      {title ? <title>{title}</title> : null}
      {children}
    </svg>
  );
}

type P = { size?: number; title?: string };

export const IconPlay = (p: P) => (
  <Svg {...p}>
    <path d="M4.5 2.8v10.4L13 8z" fill="currentColor" stroke="none" />
  </Svg>
);
export const IconPause = (p: P) => (
  <Svg {...p}>
    <path d="M4 3h3v10H4zM9 3h3v10H9z" fill="currentColor" stroke="none" />
  </Svg>
);
export const IconStop = (p: P) => (
  <Svg {...p}>
    <rect x="3.5" y="3.5" width="9" height="9" rx="1" fill="currentColor" stroke="none" />
  </Svg>
);
export const IconWarn = (p: P) => (
  <Svg {...p}>
    <path d="M8 2 1.5 13.5h13z" />
    <path d="M8 6.5v3.5M8 12v.2" />
  </Svg>
);
export const IconError = (p: P) => (
  <Svg {...p}>
    <circle cx="8" cy="8" r="6.2" />
    <path d="M5.6 5.6l4.8 4.8M10.4 5.6l-4.8 4.8" />
  </Svg>
);
export const IconInfo = (p: P) => (
  <Svg {...p}>
    <circle cx="8" cy="8" r="6.2" />
    <path d="M8 7.2v4M8 4.8v.2" />
  </Svg>
);
export const IconChevronRight = (p: P) => (
  <Svg {...p}>
    <path d="M6 3.5 10.5 8 6 12.5" />
  </Svg>
);
export const IconChevronDown = (p: P) => (
  <Svg {...p}>
    <path d="M3.5 6 8 10.5 12.5 6" />
  </Svg>
);
export const IconUp = (p: P) => (
  <Svg {...p}>
    <path d="M3.5 10 8 5.5 12.5 10" />
  </Svg>
);
export const IconFolder = (p: P) => (
  <Svg {...p}>
    <path d="M1.8 4.2c0-.6.4-1 1-1h3l1.4 1.6h6c.6 0 1 .4 1 1v6.2c0 .6-.4 1-1 1H2.8c-.6 0-1-.4-1-1z" />
  </Svg>
);
export const IconFile = (p: P) => (
  <Svg {...p}>
    <path d="M4 1.8h5l3 3v9.4H4z" />
    <path d="M9 1.8v3h3" />
  </Svg>
);
export const IconPlus = (p: P) => (
  <Svg {...p}>
    <path d="M8 3v10M3 8h10" />
  </Svg>
);
export const IconX = (p: P) => (
  <Svg {...p}>
    <path d="M3.5 3.5l9 9M12.5 3.5l-9 9" />
  </Svg>
);
export const IconUndo = (p: P) => (
  <Svg {...p}>
    <path d="M5.5 3 2.5 6l3 3" />
    <path d="M2.5 6h6.5a4 4 0 0 1 0 8H6" />
  </Svg>
);
export const IconRedo = (p: P) => (
  <Svg {...p}>
    <path d="M10.5 3l3 3-3 3" />
    <path d="M13.5 6H7a4 4 0 0 0 0 8h3" />
  </Svg>
);
export const IconSave = (p: P) => (
  <Svg {...p}>
    <path d="M2.5 2.5h8.5l2.5 2.5v8.5h-11z" />
    <path d="M5 2.5v3.5h5V2.5M5 13.5V9.5h6v4" />
  </Svg>
);
export const IconGear = (p: P) => (
  <Svg {...p}>
    <circle cx="8" cy="8" r="2.2" />
    <path d="M8 1.5v2M8 12.5v2M1.5 8h2M12.5 8h2M3.4 3.4l1.4 1.4M11.2 11.2l1.4 1.4M3.4 12.6l1.4-1.4M11.2 4.8l1.4-1.4" />
  </Svg>
);
export const IconLock = (p: P) => (
  <Svg {...p}>
    <rect x="3.5" y="7" width="9" height="6.5" rx="1" />
    <path d="M5.5 7V5a2.5 2.5 0 0 1 5 0v2" />
  </Svg>
);
export const IconPrev = (p: P) => (
  <Svg {...p}>
    <path d="M8 12.5V3.5M4 7.5l4-4 4 4" />
  </Svg>
);
export const IconNext = (p: P) => (
  <Svg {...p}>
    <path d="M8 3.5v9M4 8.5l4 4 4-4" />
  </Svg>
);
export const IconReset = (p: P) => (
  <Svg {...p}>
    <path d="M3 8a5 5 0 1 0 1.6-3.7" />
    <path d="M2.5 2.5v3h3" />
  </Svg>
);
export const IconLoaded = (p: P) => (
  <Svg {...p}>
    <path d="M3 3v10M6 4.5v7L12.5 8z" />
  </Svg>
);
export const IconDisarmed = (p: P) => (
  <Svg {...p}>
    <circle cx="8" cy="8" r="6" />
    <path d="M3.8 12.2l8.4-8.4" />
  </Svg>
);
export const IconClock = (p: P) => (
  <Svg {...p}>
    <circle cx="8" cy="8" r="6.2" />
    <path d="M8 4.5V8l2.5 1.5" />
  </Svg>
);

// ---- cue type icons ----
const TYPE_PATHS: Record<CueType, ReactNode> = {
  audio: (
    <>
      <path d="M2.5 6.2v3.6h2.4L8.2 13V3L4.9 6.2z" />
      <path d="M10.5 5.8a3 3 0 0 1 0 4.4M12.3 4a5.6 5.6 0 0 1 0 8" />
    </>
  ),
  fade: <path d="M2 3.5h3.5c3.5 0 3 9 6.5 9h2" />,
  midi: (
    <>
      <rect x="1.8" y="3.5" width="12.4" height="9" rx="1" />
      <path d="M5 3.5v5M8 3.5v5M11 3.5v5" />
    </>
  ),
  timecode: (
    <>
      <circle cx="8" cy="8.6" r="5.2" />
      <path d="M8 5.8v2.8l1.8 1.2M6.5 1.8h3" />
    </>
  ),
  control: (
    <>
      <path d="M2.5 8h7" />
      <path d="M7 4.5 10.5 8 7 11.5" />
      <path d="M13.5 3.5v9" />
    </>
  ),
  group: (
    <>
      <rect x="2" y="2.5" width="12" height="11" rx="1.2" />
      <path d="M4.8 6h6.4M4.8 8.5h6.4M4.8 11h4" />
    </>
  ),
  memo: (
    <>
      <path d="M3 2.5h10v8l-3 3H3z" />
      <path d="M13 10.5h-3v3M5.5 5.5h5M5.5 8h3.5" />
    </>
  ),
};

export function TypeIcon({ type, size = 16, title }: { type: CueType; size?: number; title?: string }) {
  return (
    <Svg size={size} title={title}>
      {TYPE_PATHS[type]}
    </Svg>
  );
}
