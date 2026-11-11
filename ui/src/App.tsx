import { useEffect } from "react";
import { ConnectionBanner, PinPad, Splash, Toasts } from "./components/Panels";
import { DesktopApp } from "./desktop/DesktopApp";
import { RemoteApp } from "./routes/RemoteApp";
import { ViewApp } from "./routes/ViewApp";
import { route } from "./session";
import { useApp } from "./store";

export function App() {
  const auth = useApp((s) => s.auth);
  const role = useApp((s) => s.role);
  const hasDoc = useApp((s) => s.doc != null);
  const highContrast = useApp((s) => s.doc?.show.settings.high_contrast ?? false);
  const lost = useApp((s) => s.conn !== "open" && s.everConnected);

  useEffect(() => {
    const root = document.documentElement;
    if (highContrast) root.setAttribute("data-contrast", "high");
    else root.removeAttribute("data-contrast");
  }, [highContrast]);

  let screen;
  if (auth === "pin") screen = <PinPad />;
  else if (auth === "unreachable") screen = <Splash text="Server not reachable – retrying…" />;
  else if (auth === "boot" || !hasDoc) screen = <Splash text="Connecting…" />;
  // What is rendered follows the role the server granted, never just the URL.
  else if (role === "view" || route === "view") screen = <ViewApp />;
  else if (role === "control" || route === "remote") screen = <RemoteApp />;
  else screen = <DesktopApp />;

  return (
    <div className={`app route-${route}${lost ? " is-offline" : ""}`}>
      <ConnectionBanner />
      <div className="app-screen">{screen}</div>
      <Toasts />
    </div>
  );
}
