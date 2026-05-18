import { useEffect, useState } from "react";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";

import { writeText } from "@tauri-apps/plugin-clipboard-manager";

import {
  changedFiles,
  fileDiff,
  takePendingDiffOpen,
  type DiffOpenPayload,
} from "../lib/ipc";
import { getDiffSelectionRange, refLineWithFile } from "../lib/smartcopy";
import type { ChangedFile } from "../types";
import { ContextMenu, type MenuItem } from "./ContextMenu";
import { ImageDiff } from "./ImageDiff";
import { SideBySideDiff } from "./SideBySideDiff";

const IMAGE_EXT = new Set([
  "png",
  "jpg",
  "jpeg",
  "gif",
  "webp",
  "svg",
  "bmp",
  "ico",
]);

function extOf(path: string): string {
  const i = path.lastIndexOf(".");
  return i >= 0 ? path.slice(i + 1).toLowerCase() : "";
}

function formatSize(bytes: number | null): string {
  if (bytes == null) return "—";
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / 1024 / 1024).toFixed(2)} MB`;
}

export function DiffApp() {
  const [ctx, setCtx] = useState<DiffOpenPayload | null>(null);
  const [files, setFiles] = useState<ChangedFile[]>([]);
  const [selectedFile, setSelectedFile] = useState<string | null>(null);
  const [diffText, setDiffText] = useState<string | null>(null);
  const [contextMenu, setContextMenu] = useState<{
    x: number;
    y: number;
    items: MenuItem[];
  } | null>(null);

  useEffect(() => {
    let un: UnlistenFn | undefined;
    let cancelled = false;

    (async () => {
      try {
        const pending = await takePendingDiffOpen();
        if (!cancelled && pending) {
          setCtx(pending);
          setSelectedFile(pending.filePath);
        }
      } catch {}

      un = await listen<DiffOpenPayload>("diff://open", (e) => {
        setCtx(e.payload);
        setSelectedFile(e.payload.filePath);
      });
    })();

    function onKey(e: KeyboardEvent) {
      if (e.key === "Escape") {
        void getCurrentWindow().hide();
      }
    }
    window.addEventListener("keydown", onKey);

    return () => {
      cancelled = true;
      un?.();
      window.removeEventListener("keydown", onKey);
    };
  }, []);

  useEffect(() => {
    if (!ctx) return;
    let cancelled = false;
    (async () => {
      try {
        const fs = await changedFiles(ctx.repoPath, ctx.hash);
        if (!cancelled) setFiles(fs);
      } catch {
        if (!cancelled) setFiles([]);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [ctx?.repoPath, ctx?.hash]);

  const selectedFileMeta: ChangedFile | undefined =
    selectedFile != null
      ? files.find((f) => f.path === selectedFile)
      : undefined;

  const isImage =
    !!selectedFile && IMAGE_EXT.has(extOf(selectedFile));
  const isBinary = selectedFileMeta?.isBinary === true;

  // Only fetch text diff if it's worth rendering.
  useEffect(() => {
    if (!ctx || !selectedFile) return;
    if (isImage || isBinary) {
      setDiffText("");
      return;
    }
    let cancelled = false;
    setDiffText(null);
    (async () => {
      try {
        const txt = await fileDiff(ctx.repoPath, ctx.hash, selectedFile);
        if (!cancelled) setDiffText(txt);
      } catch (e) {
        if (!cancelled) setDiffText(`Error: ${String(e)}`);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [ctx?.repoPath, ctx?.hash, selectedFile, isImage, isBinary]);

  function onShellContextMenu(e: React.MouseEvent) {
    const target = e.target as HTMLElement;
    if (target.closest('input, textarea, [contenteditable="true"]')) return;
    e.preventDefault();
    if (!ctx) return;
    const selection = window.getSelection()?.toString() ?? "";
    const range = getDiffSelectionRange();
    const items: MenuItem[] = [];

    if (selection) {
      items.push({
        label: "Copy",
        onClick: () => void writeText(selection),
      });
      if (selectedFile) {
        items.push({
          label: "Copy with reference",
          onClick: () => {
            const ref = refLineWithFile(
              ctx.repoName,
              ctx.shortHash,
              selectedFile,
              range?.start ?? null,
              range?.end ?? null,
            );
            void writeText(`${ref}\n${selection}`);
          },
        });
      }
      items.push({ divider: true });
    }

    if (selectedFile) {
      items.push({
        label: "Copy file path",
        onClick: () => void writeText(selectedFile),
      });
    }
    items.push({
      label: "Copy short hash",
      onClick: () => void writeText(ctx.shortHash),
    });
    items.push({
      label: "Copy full hash",
      onClick: () => void writeText(ctx.hash),
    });

    if (items.length === 0) return;
    setContextMenu({ x: e.clientX, y: e.clientY, items });
  }

  if (!ctx) {
    return <div className="diff-loading">Waiting for a file…</div>;
  }

  return (
    <div className="diff-shell" onContextMenu={onShellContextMenu}>
      <header className="diff-header">
        <div className="diff-header-summary">{ctx.summary}</div>
        <div className="diff-header-meta">
          <span>{ctx.repoName}</span>
          <code className="diff-header-hash" title={ctx.hash}>
            {ctx.shortHash}
          </code>
        </div>
      </header>
      <div className="diff-body">
        <aside className="diff-sidebar">
          {files.length === 0 ? (
            <div className="diff-sidebar-empty">Loading files…</div>
          ) : (
            files.map((f) => {
              const isSel = f.path === selectedFile;
              const slash = f.path.lastIndexOf("/");
              const dir = slash >= 0 ? f.path.slice(0, slash + 1) : "";
              const name = slash >= 0 ? f.path.slice(slash + 1) : f.path;
              return (
                <button
                  key={f.path}
                  className={"diff-file" + (isSel ? " active" : "")}
                  onClick={() => setSelectedFile(f.path)}
                  title={f.path}
                >
                  <div className="diff-file-line">
                    <span className="diff-file-name">{name}</span>
                    {f.isBinary && (
                      <span className="changed-file-bin" title="Binary file">
                        bin
                      </span>
                    )}
                    <span className="diff-file-stat">
                      {f.isBinary ? (
                        <span className="diff-file-binsize">
                          {formatSize(f.newSize ?? f.oldSize)}
                        </span>
                      ) : (
                        <>
                          <span className="changed-file-plus">
                            +{f.insertions}
                          </span>
                          <span className="changed-file-minus">
                            −{f.deletions}
                          </span>
                        </>
                      )}
                    </span>
                  </div>
                  {dir && <div className="diff-file-dir">{dir}</div>}
                </button>
              );
            })
          )}
        </aside>
        <main className="diff-main">
          {!selectedFile ? (
            <div className="diff-loading">Pick a file.</div>
          ) : isImage ? (
            <ImageDiff
              repoPath={ctx.repoPath}
              hash={ctx.hash}
              filePath={selectedFile}
              oldPath={selectedFileMeta?.oldPath ?? null}
              oldSize={selectedFileMeta?.oldSize ?? null}
              newSize={selectedFileMeta?.newSize ?? null}
            />
          ) : isBinary ? (
            <div className="binary-info">
              <div className="binary-info-title">Binary file</div>
              <div className="binary-info-meta">
                {formatSize(selectedFileMeta?.oldSize ?? null)} →{" "}
                {formatSize(selectedFileMeta?.newSize ?? null)}
              </div>
              <div className="binary-info-hint">
                gitwink doesn't render diffs for non-image binaries yet.
              </div>
            </div>
          ) : diffText == null ? (
            <div className="diff-loading">Loading diff…</div>
          ) : (
            <SideBySideDiff text={diffText} />
          )}
        </main>
      </div>
      {contextMenu && (
        <ContextMenu
          items={contextMenu.items}
          x={contextMenu.x}
          y={contextMenu.y}
          onClose={() => setContextMenu(null)}
        />
      )}
    </div>
  );
}
