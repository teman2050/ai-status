import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import Panel from "./Panel";
import "./styles.css";

// Block the WebView's default context menu (Reload / Inspect Element) — a shipped app shouldn't leak the browser menu.
// Both windows load this file, so it applies to the floating widget and the settings panel.
window.addEventListener("contextmenu", (e) => e.preventDefault());

const surface = new URLSearchParams(window.location.search).get("surface");
const Root = surface === "panel" ? Panel : App;

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <Root />
  </React.StrictMode>,
);
