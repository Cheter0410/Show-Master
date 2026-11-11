import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { App } from "./App";
import { boot, route } from "./session";
import "./styles.css";

document.documentElement.setAttribute("data-route", route);

// iOS Safari ignores `user-scalable=no`; block pinch zoom on the control surfaces.
if (route !== "desktop") {
  document.addEventListener("gesturestart", (e) => e.preventDefault());
}

const el = document.getElementById("root");
if (el) {
  createRoot(el).render(
    <StrictMode>
      <App />
    </StrictMode>,
  );
}
void boot();
