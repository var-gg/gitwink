# gitwink

> Tray-resident, read-only git glance for the AI-agent era.

**Status:** v0.1 — bootstrapping. Not yet usable.

gitwink is a small desktop tool that lives in your system tray. Click it to
glance at recent commit activity across **all** your local repos. It is **not**
a git client — it cannot commit, push, merge, or modify anything. Read-only by
design.

The 0.5-second confirm loop: agent commits → you click the tray → you see
what changed → optionally "Copy as AI context" → paste into Claude/Codex →
ask "did the agent do this right?"

## Tech

Tauri 2 · Rust · React + TypeScript · `git2` · SQLite

## Development

```bash
pnpm install
pnpm tauri dev
```

Requires: Node 20+, Rust stable (msvc on Windows), Visual C++ Build Tools
(Windows) or Xcode CLT (macOS).

## Platforms

- Windows 10/11 (primary)
- macOS 13+ (secondary)
- Linux: not yet

## License

[MIT](LICENSE)
