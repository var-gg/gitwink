import { getCurrentWindow } from "@tauri-apps/api/window";
import "./styles.css";

function startDrag(e: React.MouseEvent) {
  if (e.buttons !== 1) return;
  void getCurrentWindow().startDragging();
}

function App() {
  return (
    <main className="panel">
      <header className="panel-header" onMouseDown={startDrag}>
        <h1>gitwink</h1>
        <span className="panel-status">v0.1 — bootstrapping</span>
      </header>
      <section className="panel-body">
        <p className="panel-empty">Timeline lands in D4.</p>
      </section>
    </main>
  );
}

export default App;
