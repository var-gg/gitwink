import { useEffect, useRef, useState } from "react";
import { useLingui } from "@lingui/react/macro";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";

import { writeText } from "@tauri-apps/plugin-clipboard-manager";

import {
  changedFiles,
  fileDiff,
  takePendingDiffOpen,
  WHOLE_FILE_CONTEXT,
  type DiffOpenPayload,
} from "../lib/ipc";
import { patchForClipboard } from "../lib/contextBudget";
import { DiffFileList } from "./DiffFileList";
import "./diffFeedback.css";
import { fileMenuItems } from "../lib/commitClipboard";
import { getDiffSelectionRange, refLineWithFile } from "../lib/smartcopy";
import type { ChangedFile, FilePatch } from "../types";
import { ContextMenu, type MenuItem } from "./ContextMenu";
import { ImageDiff } from "./ImageDiff";
import { SideBySideDiff } from "./SideBySideDiff";
import { diffMessages } from "../i18n/messages/diff";
import { translate } from "../i18n/translate";
import { commitMenuLabels } from "../i18n/clipboard";

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

/** Above this combined (old+new) size, "Full" context is disabled — git
 *  would otherwise emit a multi-MB patch that floods IPC + the parser + the
 *  DOM. The backend also caps the emitted patch as a second line of defense. */
const FULL_MAX_BYTES = 1_500_000;

/** Header context-toggle steps. "Full" expands git's context past any real
 * file length, so the same side-by-side view renders the whole file with
 * changes still tinted — no separate read-only editor. */
/** Vertical scroll lock for the side-by-side view. Locked (default) scrolls
 * both columns as one; unlocked lets the old/new sides roam independently.
 * Persisted so the choice sticks across files and sessions. */
const SCROLL_LOCK_KEY = "gitwink.diffScrollLock";
function loadScrollLock(): boolean {
  if (typeof window === "undefined") return true;
  return window.localStorage.getItem(SCROLL_LOCK_KEY) !== "0";
}

/** The diff-context choice (±3 / ±25 / Full) persists across files and
 * sessions, like the scroll lock — picking "Full" once shouldn't reset to ±3
 * on the next file. Oversized files are still clamped at render time via
 * `effectiveContext`, so a carried-over "Full" never fires a huge fetch. */
const CONTEXT_KEY = "gitwink.diffContext";
function loadContext(): number {
  if (typeof window === "undefined") return 3;
  const v = parseInt(window.localStorage.getItem(CONTEXT_KEY) ?? "", 10);
  return v === 3 || v === 25 || v === WHOLE_FILE_CONTEXT ? v : 3;
}

export function DiffApp() {
  const { i18n } = useLingui();
  const menuLabels = commitMenuLabels(i18n);
  const contextOptions: { value: number; label: string; title: string }[] = [
    {
      value: 3,
      label: "±3",
      title: i18n._(diffMessages.contextDefault),
    },
    {
      value: 25,
      label: "±25",
      title: i18n._(diffMessages.contextExpanded),
    },
    {
      value: WHOLE_FILE_CONTEXT,
      label: i18n._(diffMessages.contextFull),
      title: i18n._(diffMessages.contextFullTitle),
    },
  ];
  const [ctx, setCtx] = useState<DiffOpenPayload | null>(null);
  const [files, setFiles] = useState<ChangedFile[]>([]);
  // True total changed-file count — the shipped list is capped backend-side
  // for vendor-bump-scale commits ("showing N of M" in the sidebar).
  const [filesTotal, setFilesTotal] = useState(0);
  const [selectedFile, setSelectedFile] = useState<string | null>(null);
  const [patch, setPatch] = useState<FilePatch | null>(null);
  const diffText = patch?.text ?? null;
  // A real diff-fetch failure, kept SEPARATE from diffText so an error never
  // gets parsed as a (zero-hunk) patch and rendered as a misleading
  // "No textual diff." When set, the main pane shows an honest error block.
  const [diffError, setDiffError] = useState<string | null>(null);
  // True while a text diff is in flight. During a same-file context toggle
  // the old text stays on screen (no unmount), so this drives a small
  // floating chip instead of the full "Loading diff…" swap.
  const [diffLoading, setDiffLoading] = useState(false);
  // Which repo:hash:file the CURRENT diffText belongs to — a same-key fetch
  // (context toggle) replaces in place; a different key unmounts to the
  // loading state first so another file's text never lingers.
  const shownDiffKeyRef = useRef("");
  // Unified-diff context lines for the selected file. 3 = default hunk
  // view; the header toggle bumps it to expand context or show the whole
  // file. Persists across file switches so "Full" stays on if chosen.
  const [context, setContext] = useState<number>(loadContext);
  // Which (repo,hash) the loaded `files` metadata belongs to, so the
  // text-diff effect never fetches using a previous commit's metadata.
  const [filesCtx, setFilesCtx] = useState<string | null>(null);
  // True when changedFiles() for the current commit FAILED (vs. a commit that
  // genuinely changed nothing) — lets the sidebar tell those two apart instead
  // of sitting on "Loading files…" forever.
  const [filesError, setFilesError] = useState(false);
  const [contextMenu, setContextMenu] = useState<{
    x: number;
    y: number;
    items: MenuItem[];
  } | null>(null);
  const [scrollLocked, setScrollLocked] = useState(loadScrollLock);

  function toggleScrollLock() {
    setScrollLocked((v) => {
      const next = !v;
      try {
        window.localStorage.setItem(SCROLL_LOCK_KEY, next ? "1" : "0");
      } catch {}
      return next;
    });
  }

  // Pick a diff-context level and remember it across files/sessions.
  function chooseContext(v: number) {
    setContext(v);
    try {
      window.localStorage.setItem(CONTEXT_KEY, String(v));
    } catch {}
  }

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

      try {
        const u = await listen<DiffOpenPayload>("diff://open", (e) => {
          setCtx(e.payload);
          setSelectedFile(e.payload.filePath);
        });
        // Unmounted before listen resolved → unsubscribe immediately so the
        // handler can't fire on a dead component.
        if (cancelled) u();
        else un = u;
      } catch {
        /* listener registration failed — nothing to unsubscribe */
      }
    })();

    function onKey(e: KeyboardEvent) {
      // The Esc that cancels an IME composition must not hide the window.
      if (e.isComposing) return;
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
    const key = `${ctx.repoPath}:${ctx.hash}`;
    let cancelled = false;
    // Clear stale metadata up front so selectedFileMeta can't resolve against
    // the previous commit's files while this loads — filesTotal included, or
    // the sidebar footer would flash "Showing 0 of N" with the OLD commit's N.
    setFiles([]);
    setFilesTotal(0);
    setFilesCtx(null);
    setFilesError(false);
    (async () => {
      try {
        const r = await changedFiles(ctx.repoPath, ctx.hash);
        if (!cancelled) {
          setFiles(r.files);
          setFilesTotal(r.total);
          setFilesCtx(key);
          setFilesError(false);
        }
      } catch {
        if (!cancelled) {
          setFiles([]);
          setFilesTotal(0);
          setFilesCtx(key);
          setFilesError(true);
        }
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
  const isFullTooBig =
    !!selectedFileMeta &&
    (selectedFileMeta.oldSize ?? 0) + (selectedFileMeta.newSize ?? 0) >
      FULL_MAX_BYTES;

  // What we actually fetch/show. A carried-over "Full" is clamped to expanded
  // on a large file WITHOUT losing the user's choice (it returns when they
  // view a smaller file). Deriving it — rather than setContext(25) in an
  // effect — avoids the one-frame Full request the demotion used to let slip.
  const effectiveContext =
    context === WHOLE_FILE_CONTEXT && isFullTooBig ? 25 : context;

  // (No per-commit context reset: the chosen level persists across files and
  // sessions via chooseContext/loadContext; effectiveContext clamps oversized
  // files so a carried-over "Full" stays safe.)

  // Only fetch text diff if it's worth rendering.
  useEffect(() => {
    if (!ctx || !selectedFile) return;
    // A fresh file / commit / context starts from a clean error slate.
    setDiffError(null);
    // Wait until `files` metadata for THIS commit has loaded — otherwise we'd
    // fetch a (possibly Full-context) text diff for a file we don't yet know
    // is binary or oversized.
    if (filesCtx !== `${ctx.repoPath}:${ctx.hash}`) {
      setPatch(null);
      shownDiffKeyRef.current = "";
      return;
    }
    if (isImage || isBinary) {
      setPatch(null);
      shownDiffKeyRef.current = "";
      return;
    }
    // Metadata loaded for this commit but the selected path isn't in it
    // (changedFiles failed or omitted it) — don't fire a text diff (least of
    // all a Full one) for a file we can't size-check.
    if (!selectedFileMeta) {
      setPatch(null);
      shownDiffKeyRef.current = "";
      return;
    }
    let cancelled = false;
    // A context toggle on the SAME file swaps the text IN PLACE — the old
    // content stays rendered (SideBySideDiff stays mounted, keeping the
    // reading position + find state) with a small loading chip. Only a
    // different file/commit goes through the unmounting "Loading diff…" swap.
    const fetchKey = `${ctx.repoPath}:${ctx.hash}:${selectedFile}`;
    if (shownDiffKeyRef.current !== fetchKey) setPatch(null);
    setDiffLoading(true);
    (async () => {
      try {
        const txt = await fileDiff(
          ctx.repoPath,
          ctx.hash,
          selectedFile,
          effectiveContext,
        );
        if (!cancelled) {
          setPatch(txt);
          shownDiffKeyRef.current = fetchKey;
        }
      } catch (e) {
        if (!cancelled) setDiffError(String(e));
      } finally {
        if (!cancelled) setDiffLoading(false);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [
    ctx?.repoPath,
    ctx?.hash,
    selectedFile,
    isImage,
    isBinary,
    effectiveContext,
    filesCtx,
    selectedFileMeta,
  ]);

  function onShellContextMenu(e: React.MouseEvent) {
    const target = e.target as HTMLElement;
    if (target.closest('input, textarea, [contenteditable="true"]')) return;
    e.preventDefault();
    if (!ctx) return;

    // The menu opens over the diff content OR a sidebar file row. A
    // right-clicked row names its own file; over the content the subject is
    // the file currently shown. The header opens nothing (the native menu
    // stays suppressed by the preventDefault above).
    const rowEl = target.closest<HTMLElement>(
      ".diff-file-wrap[data-file-path]",
    );
    const inMain = !!target.closest(".diff-main");
    if (!rowEl && !inMain) return;
    const fileForMenu = rowEl?.dataset.filePath ?? selectedFile;

    const items: MenuItem[] = [];

    // Selection section — only meaningful over the rendered diff text.
    const selection = inMain ? window.getSelection()?.toString() ?? "" : "";
    if (selection) {
      const range = getDiffSelectionRange();
      items.push({
        label: i18n._(diffMessages.copy),
        onClick: () => void writeText(selection),
      });
      if (fileForMenu) {
        const file = fileForMenu;
        items.push({
          label: i18n._(diffMessages.copyWithReference),
          onClick: () => {
            const ref = !range && patch ? `${patch.oldRevision}:${patch.oldPath} → ${patch.newRevision}:${patch.newPath}` : refLineWithFile(
              ctx.repoName,
              (range?.side === "left" ? patch?.oldRevision : patch?.newRevision) ?? ctx.hash,
              (range?.side === "left" ? patch?.oldPath : patch?.newPath) ?? file,
              range?.start ?? null,
              range?.end ?? null,
            );
            void writeText(`${ref}\n${selection}`);
          },
        });
      }
      items.push({ divider: true });
    }

    // File section — history + path for the file under the cursor.
    const fileItems = fileMenuItems(
      ctx.repoPath,
      fileForMenu ?? null,
      menuLabels,
    );
    if (fileItems.length) {
      items.push(...fileItems);
      // Copy the patch the user is reading — diff window only, and only for
      // the file actually shown (we hold no other file's diff text). Prefixed
      // with the same reference header as "Copy with reference" so it drops
      // straight into a chat prompt.
      if (
        selectedFile &&
        fileForMenu === selectedFile &&
        diffText &&
        diffText.trim() &&
        !isImage &&
        !isBinary
      ) {
        const clipboardPatch = patch ? patchForClipboard(patch) : diffText;
        const header = refLineWithFile(
          ctx.repoName,
          ctx.shortHash,
          selectedFile,
          null,
          null,
        );
        items.push({
          label: i18n._(diffMessages.copyFileDiff),
          onClick: () => void writeText(`${header}\n${clipboardPatch}`),
        });
      }
      items.push({ divider: true });
    }

    // Commit section.
    items.push({
      label: i18n._(diffMessages.copyShortHash),
      onClick: () => void writeText(ctx.shortHash),
    });
    items.push({
      label: i18n._(diffMessages.copyFullHash),
      onClick: () => void writeText(ctx.hash),
    });

    if (items.length === 0) return;
    setContextMenu({ x: e.clientX, y: e.clientY, items });
  }

  if (!ctx) {
    return (
      <div className="diff-loading">{i18n._(diffMessages.waitingForFile)}</div>
    );
  }

  return (
    <div className="diff-shell" onContextMenu={onShellContextMenu}>
      <header className="diff-header">
        <div className="diff-header-left">
          <div className="diff-header-summary">{ctx.summary}</div>
          <div className="diff-header-meta">
            <span>{ctx.repoName}</span>
            <code className="diff-header-hash" title={ctx.hash}>
              {ctx.shortHash}
            </code>
          </div>
        </div>
        {selectedFile && !isImage && !isBinary && (
          <div className="diff-header-tools">
            <div
              className="diff-context-toggle"
              role="group"
              aria-label={i18n._(diffMessages.context)}
            >
              {contextOptions.map((o) => {
                const disabled =
                  o.value === WHOLE_FILE_CONTEXT && isFullTooBig;
                return (
                  <button
                    key={o.value}
                    type="button"
                    className={
                      "diff-context-btn" +
                      (effectiveContext === o.value ? " active" : "")
                    }
                    disabled={disabled}
                    onClick={() => chooseContext(o.value)}
                    title={
                      disabled ? i18n._(diffMessages.contextTooLarge) : o.title
                    }
                  >
                    {o.label}
                  </button>
                );
              })}
            </div>
            <button
              type="button"
              className={"diff-sync-btn" + (scrollLocked ? " active" : "")}
              onClick={toggleScrollLock}
              aria-pressed={scrollLocked}
              title={
                scrollLocked
                  ? i18n._(diffMessages.syncOn)
                  : i18n._(diffMessages.syncOff)
              }
            >
              ⇅ {i18n._(diffMessages.sync)}
            </button>
          </div>
        )}
      </header>
      <div className="diff-body">
        <aside className="diff-sidebar">
          {filesCtx !== `${ctx.repoPath}:${ctx.hash}` ? (
            <div className="diff-sidebar-empty">
              {i18n._(diffMessages.loadingFiles)}
            </div>
          ) : filesError ? (
            <div className="diff-sidebar-empty">
              {i18n._(diffMessages.filesLoadFailed)}
            </div>
          ) : files.length === 0 ? (
            <div className="diff-sidebar-empty">
              {i18n._(diffMessages.noFileChanges)}
            </div>
          ) : (
            <DiffFileList files={files} selectedPath={selectedFile} onSelect={setSelectedFile} repoPath={ctx.repoPath} />
          )}
          {filesTotal > files.length && (
            <div className="diff-sidebar-more">
              {translate(i18n, diffMessages.showingChangedFiles, {
                shown: files.length.toLocaleString(i18n.locale),
                total: filesTotal.toLocaleString(i18n.locale),
              })}
            </div>
          )}
        </aside>
        <main className="diff-main">
          {!selectedFile ? (
            <div className="diff-loading">{i18n._(diffMessages.pickFile)}</div>
          ) : filesError ? (
            // The commit's file list failed to load, so we have no metadata for
            // this file — say so plainly instead of letting an empty diff read
            // as "No textual diff." (i.e. "nothing changed"). Mirrors the
            // sidebar's honest error.
            <div className="binary-info">
              <div className="binary-info-title">
                {i18n._(diffMessages.filesLoadFailedTitle)}
              </div>
              <div className="binary-info-hint">
                {i18n._(diffMessages.cannotShow)}
              </div>
            </div>
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
              <div className="binary-info-title">
                {i18n._(diffMessages.binaryFile)}
              </div>
              <div className="binary-info-meta">
                {formatSize(selectedFileMeta?.oldSize ?? null)} →{" "}
                {formatSize(selectedFileMeta?.newSize ?? null)}
              </div>
              <div className="binary-info-hint">
                {i18n._(diffMessages.binaryUnsupported)}
              </div>
            </div>
          ) : diffError ? (
            <div className="binary-info">
              <div className="binary-info-title">
                {i18n._(diffMessages.loadFailed)}
              </div>
              <div className="binary-info-hint">{diffError}</div>
            </div>
          ) : diffText == null ? (
            <div className="diff-loading">{i18n._(diffMessages.loading)}</div>
          ) : (
            <>
              {diffLoading && (
                <div className="diff-inline-loading" role="status">
                  {i18n._(diffMessages.loadingShort)}
                </div>
              )}
              {patch?.truncated && <div className="diff-limit-notice" role="status">{i18n._(diffMessages.patchTruncated)}</div>}
              <SideBySideDiff
                key={`${ctx.repoPath}:${ctx.hash}:${selectedFile}`}
                text={diffText}
                filePath={selectedFile}
                fileKey={`${ctx.repoPath}:${ctx.hash}:${selectedFile}`}
                locked={scrollLocked}
              />
            </>
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
