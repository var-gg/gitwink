import React from "react";
import ReactDOM from "react-dom/client";
import { getCurrentWindow } from "@tauri-apps/api/window";

import App from "./App";
import { DiffApp } from "./components/DiffApp";
import { Settings } from "./components/Settings";
import { initSettings } from "./lib/settings";

const label = getCurrentWindow().label;
const Root =
  label === "diff" ? DiffApp : label === "settings" ? Settings : App;

/** Catches render errors so a webview never goes silently blank — shows
 *  the error text instead, which we (and the user) can read directly in
 *  the window. Without this, a thrown exception during the initial
 *  render unmounts the tree and leaves a pure-white window with no
 *  hint of what happened. Errors are also logged to the webview console
 *  for devtools / future capture. */
class RootErrorBoundary extends React.Component<
  { children: React.ReactNode },
  { error: Error | null }
> {
  state: { error: Error | null } = { error: null };
  static getDerivedStateFromError(error: Error) {
    return { error };
  }
  componentDidCatch(error: Error, info: React.ErrorInfo) {
    // eslint-disable-next-line no-console
    console.error(`[gitwink] render error in ${label} window`, error, info);
  }
  render() {
    if (this.state.error) {
      return (
        <pre
          style={{
            margin: 0,
            padding: "20px",
            color: "rgba(190, 50, 50, 1)",
            fontSize: "12px",
            lineHeight: 1.5,
            fontFamily:
              'ui-monospace, SFMono-Regular, "Cascadia Mono", Menlo, monospace',
            whiteSpace: "pre-wrap",
            wordBreak: "break-word",
          }}
        >
          gitwink: render error in "{label}" window
          {"\n\n"}
          {this.state.error.message}
          {"\n\n"}
          {this.state.error.stack}
        </pre>
      );
    }
    return this.props.children;
  }
}

// Load + apply persisted settings before first paint so the font / scale
// are already correct — no flash of default styling.
void initSettings().finally(() => {
  ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
    <React.StrictMode>
      <RootErrorBoundary>
        <Root />
      </RootErrorBoundary>
    </React.StrictMode>,
  );
});
