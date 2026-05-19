import { Fragment, useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";

import { colorForBranch } from "../lib/colors";
import { computeLanes } from "../lib/lanes";
import type { BranchInfo, CommitSummary } from "../types";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";

import { buildAiContext } from "../lib/copy";
import { changedFiles, fileDiff, openDiff, prefetchCommit } from "../lib/ipc";
import { refLine, refLineWithFile } from "../lib/smartcopy";
import { ChangedFiles } from "./ChangedFiles";
import { CommitDetail } from "./CommitDetail";
import { ContextMenu, type MenuItem } from "./ContextMenu";
import { LaneGraph } from "./LaneGraph";

interface Props {
  commits: CommitSummary[];
  mode: "all" | "single";
  /** In "all" mode, clicking the repo cell jumps to single-repo mode. */
  onSelectRepo?: (repoPath: string) => void;
  /** Single-repo mode: list of branches so we can color by branch identity. */
  branches?: BranchInfo[];
  /** SHAs (formatted "repoPath:hash") that arrived via the file watcher
   * since the user last closed the panel — render with a fresh marker. */
  freshHashes?: Set<string>;
}

function timeAgo(unixSeconds: number): string {
  const now = Math.floor(Date.now() / 1000);
  const diff = Math.max(0, now - unixSeconds);
  if (diff < 60) return `${diff}s`;
  if (diff < 3600) return `${Math.floor(diff / 60)}m`;
  if (diff < 86_400) return `${Math.floor(diff / 3600)}h`;
  return `${Math.floor(diff / 86_400)}d`;
}

function marker(c: CommitSummary): { glyph: string; cls: string; title: string } {
  if (c.isTagged) return { glyph: "★", cls: "marker-tag", title: "Tagged commit" };
  if (c.isMerge) return { glyph: "◆", cls: "marker-merge", title: "Merge commit" };
  return { glyph: "●", cls: "marker-dot", title: "Commit" };
}

export function Timeline({
  commits,
  mode,
  onSelectRepo,
  branches,
  freshHashes,
}: Props) {
  const [selected, setSelected] = useState(0);
  const [expandedHash, setExpandedHash] = useState<string | null>(null);
  const [copyStatus, setCopyStatus] = useState<"idle" | "copied" | "error">(
    "idle",
  );
  const listRef = useRef<HTMLUListElement | null>(null);
  const rowRefs = useRef<(HTMLLIElement | null)[]>([]);
  const [rowYs, setRowYs] = useState<number[]>([]);
  const hoverTimers = useRef(new Map<string, number>());
  const [contextMenu, setContextMenu] = useState<{
    x: number;
    y: number;
    items: MenuItem[];
  } | null>(null);

  rowRefs.current.length = commits.length;

  useEffect(() => {
    if (selected > commits.length - 1) setSelected(Math.max(0, commits.length - 1));
  }, [commits.length, selected]);

  // Reset expansion when the commit list itself changes (e.g. filter swap).
  useEffect(() => {
    setExpandedHash(null);
  }, [commits]);

  const toggleExpand = useCallback(
    (hash: string) => {
      setExpandedHash((cur) => (cur === hash ? null : hash));
    },
    [],
  );

  const copyAiContext = useCallback(async (commit: CommitSummary) => {
    try {
      const files = await changedFiles(commit.repoPath, commit.hash);
      let diffText: string | null = null;
      const TOTAL_LINES = files.reduce(
        (a, f) => a + (f.isBinary ? 0 : f.insertions + f.deletions),
        0,
      );
      // Pull full diff only when it's small enough to be useful in a chat
      // prompt; bigger commits get a file list summary only.
      if (!files.some((f) => f.isBinary) && TOTAL_LINES <= 800) {
        try {
          const parts: string[] = [];
          for (const f of files) {
            const t = await fileDiff(commit.repoPath, commit.hash, f.path);
            parts.push(`--- ${f.path}\n${t}`);
          }
          diffText = parts.join("\n");
        } catch {
          diffText = null;
        }
      }
      const md = buildAiContext(commit, files, diffText);
      await writeText(md);
      setCopyStatus("copied");
      setTimeout(() => setCopyStatus("idle"), 1500);
    } catch {
      setCopyStatus("error");
      setTimeout(() => setCopyStatus("idle"), 2000);
    }
  }, []);

  useEffect(() => {
    function onKey(e: KeyboardEvent) {
      const target = e.target as HTMLElement | null;
      if (target && ["INPUT", "TEXTAREA"].includes(target.tagName)) return;
      if (e.key === "j" || e.key === "ArrowDown") {
        setSelected((s) => Math.min(s + 1, commits.length - 1));
        e.preventDefault();
      } else if (e.key === "k" || e.key === "ArrowUp") {
        setSelected((s) => Math.max(s - 1, 0));
        e.preventDefault();
      } else if (e.key === "Enter") {
        const c = commits[selected];
        if (c) toggleExpand(c.hash);
        e.preventDefault();
      } else if (e.key === "c" || e.key === "C") {
        const c = commits[selected];
        if (c) {
          void copyAiContext(c);
          e.preventDefault();
        }
      } else if (e.key === "Escape" && expandedHash != null) {
        setExpandedHash(null);
        e.preventDefault();
        // Block other Esc handlers (App-level panel hide) from firing
        // when we've consumed the keystroke to collapse the expansion.
        e.stopImmediatePropagation();
      }
    }
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [commits, selected, toggleExpand, expandedHash, copyAiContext]);

  // Clean up any pending hover timers when the commit list changes.
  useEffect(() => {
    const timers = hoverTimers.current;
    return () => {
      for (const id of timers.values()) {
        window.clearTimeout(id);
      }
      timers.clear();
    };
  }, [commits]);

  function onRowEnter(c: CommitSummary) {
    const key = `${c.repoPath}:${c.hash}`;
    if (hoverTimers.current.has(key)) return;
    const id = window.setTimeout(() => {
      void prefetchCommit(c.repoPath, c.hash);
      hoverTimers.current.delete(key);
    }, 200);
    hoverTimers.current.set(key, id);
  }

  function onRowLeave(c: CommitSummary) {
    const key = `${c.repoPath}:${c.hash}`;
    const id = hoverTimers.current.get(key);
    if (id != null) {
      window.clearTimeout(id);
      hoverTimers.current.delete(key);
    }
  }

  function onTimelineContextMenu(e: React.MouseEvent) {
    const target = e.target as HTMLElement;
    if (target.closest('input, textarea, [contenteditable="true"]')) return;
    e.preventDefault();

    // Identify the commit this click is on. Direct hit on a timeline row
    // wins; otherwise inherit from the currently-expanded commit (the
    // user is clicking inside its expansion: commit detail or file list).
    const row = target.closest<HTMLLIElement>("[data-row]");
    const idx = row ? parseInt(row.dataset.row ?? "-1", 10) : -1;
    let commit: CommitSummary | null = idx >= 0 ? commits[idx] : null;
    if (!commit && expandedHash) {
      commit = commits.find((c) => c.hash === expandedHash) ?? null;
    }

    const fileEl = target.closest<HTMLElement>("[data-file-path]");
    const filePath = fileEl?.dataset.filePath ?? null;

    const selection = window.getSelection()?.toString() ?? "";
    const items: MenuItem[] = [];

    if (selection) {
      items.push({
        label: "Copy",
        onClick: () => void writeText(selection),
      });
      if (commit) {
        const ref = filePath
          ? refLineWithFile(
              commit.repoName,
              commit.shortHash,
              filePath,
              null,
              null,
            )
          : refLine(commit.repoName, commit.shortHash);
        items.push({
          label: "Copy with reference",
          onClick: () => void writeText(`${ref}\n${selection}`),
        });
      }
      items.push({ divider: true });
    }

    if (filePath) {
      items.push({
        label: "Copy file path",
        onClick: () => void writeText(filePath),
      });
    }

    if (commit) {
      items.push({
        label: "Copy as AI context",
        onClick: () => void copyAiContext(commit),
      });
      const messageText = (commit.message || commit.summary).trim();
      if (messageText) {
        items.push({
          label: "Copy commit message",
          onClick: () => void writeText(messageText),
        });
      }
      items.push({
        label: "Copy short hash",
        onClick: () => void writeText(commit.shortHash),
      });
      items.push({
        label: "Copy full hash",
        onClick: () => void writeText(commit.hash),
      });
    }

    if (items.length === 0) return;
    setContextMenu({ x: e.clientX, y: e.clientY, items });
  }

  useEffect(() => {
    const row = listRef.current?.querySelector<HTMLLIElement>(
      `[data-row="${selected}"]`,
    );
    row?.scrollIntoView({ block: "nearest" });
  }, [selected]);

  // Measure each commit row's vertical center for the lane SVG. We watch
  // the list itself with a ResizeObserver so the DAG re-aligns whenever
  // an inline expansion is added or collapsed — synchronous measurement
  // via useLayoutEffect alone was racing with React's commit/layout cycle
  // when the expansion li mounted.
  useLayoutEffect(() => {
    const list = listRef.current;
    if (!list) return;

    function measure() {
      const ys: number[] = [];
      for (let i = 0; i < rowRefs.current.length; i++) {
        const el = rowRefs.current[i];
        if (el) ys[i] = el.offsetTop + el.offsetHeight / 2;
      }
      setRowYs(ys);
    }

    measure();
    const observer = new ResizeObserver(() => measure());
    observer.observe(list);
    return () => observer.disconnect();
  }, [commits, mode]);

  const showRepo = mode === "all";
  const headBranch = branches?.find((b) => b.isHead)?.name ?? null;

  const laneGraph = useMemo(() => {
    if (mode !== "single") return null;
    return computeLanes(commits, (c) =>
      colorForBranch(c.branchLabel ?? headBranch),
    );
  }, [commits, mode, headBranch]);

  if (commits.length === 0) {
    return <p className="panel-empty">No commits match.</p>;
  }

  return (
    <ul
      className={"timeline timeline-" + mode}
      ref={listRef}
      onContextMenu={onTimelineContextMenu}
    >
      {laneGraph && rowYs.length === commits.length && (
        <LaneGraph graph={laneGraph} rowYs={rowYs} />
      )}
      {contextMenu && (
        <ContextMenu
          items={contextMenu.items}
          x={contextMenu.x}
          y={contextMenu.y}
          onClose={() => setContextMenu(null)}
        />
      )}
      {commits.map((c, i) => {
        const m = marker(c);
        return (
          <Fragment key={`${c.repoPath}:${c.hash}`}>
            <li
              data-row={i}
              ref={(el) => {
                rowRefs.current[i] = el;
              }}
              className={
                "timeline-row" +
                (i === selected ? " selected" : "") +
                (expandedHash === c.hash ? " expanded" : "")
              }
              onClick={() => {
                setSelected(i);
                toggleExpand(c.hash);
              }}
              onMouseEnter={() => onRowEnter(c)}
              onMouseLeave={() => onRowLeave(c)}
            >
              {mode === "single" ? (
                <span className="timeline-lane-spacer" aria-hidden="true" />
              ) : (
                <span
                  className={
                    "timeline-marker " +
                    m.cls +
                    (freshHashes?.has(`${c.repoPath}:${c.hash}`) ? " fresh" : "")
                  }
                  title={
                    freshHashes?.has(`${c.repoPath}:${c.hash}`)
                      ? `${m.title} (new)`
                      : m.title
                  }
                >
                  {m.glyph}
                </span>
              )}
              <span className="timeline-time">{timeAgo(c.timestamp)}</span>
              {showRepo && (
                <span
                  className={
                    "timeline-repo" +
                    (onSelectRepo ? " timeline-repo-clickable" : "")
                  }
                  title={`${c.repoPath} (click to filter)`}
                  onClick={(e) => {
                    if (!onSelectRepo) return;
                    e.stopPropagation();
                    onSelectRepo(c.repoPath);
                  }}
                >
                  {c.repoName}
                </span>
              )}
              <span className="timeline-summary" title={c.summary}>
                {c.branchLabel && (
                  <span className="timeline-branch">[{c.branchLabel}]</span>
                )}
                {c.summary}
                {c.remoteTipLabel && (
                  <span
                    className="timeline-remote-tip"
                    title={
                      c.remoteTipExtraCount > 0
                        ? `Remote tracking refs at this commit (read-only, from your last git fetch). +${c.remoteTipExtraCount} more.`
                        : "Remote tracking ref at this commit (read-only, from your last git fetch)."
                    }
                  >
                    {c.remoteTipLabel}
                    {c.remoteTipExtraCount > 0 && ` +${c.remoteTipExtraCount}`}
                  </span>
                )}
              </span>
              <span className="timeline-author" title={c.email}>
                {c.author}
              </span>
            </li>
            {expandedHash === c.hash && (
              <li className="timeline-expansion" onClick={(e) => e.stopPropagation()}>
                <CommitDetail commit={c} />
                <div className="commit-actions">
                  <button
                    type="button"
                    className="commit-copy-btn"
                    onClick={() => void copyAiContext(c)}
                    title="Copy as AI context (c)"
                  >
                    {copyStatus === "copied"
                      ? "Copied ✓"
                      : copyStatus === "error"
                        ? "Copy failed"
                        : "Copy as AI context"}
                  </button>
                </div>
                <ChangedFiles
                  repoPath={c.repoPath}
                  hash={c.hash}
                  onOpenDiff={(f) => {
                    void openDiff(
                      c.repoPath,
                      c.repoName,
                      c.hash,
                      c.shortHash,
                      c.summary,
                      f.path,
                    );
                  }}
                />
              </li>
            )}
          </Fragment>
        );
      })}
    </ul>
  );
}
