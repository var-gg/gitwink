import { useEffect, useRef, useState } from "react";

import type { CommitSummary } from "../types";

interface Props {
  commits: CommitSummary[];
  mode: "all" | "single";
  /** In "all" mode, clicking the repo cell jumps to single-repo mode. */
  onSelectRepo?: (repoPath: string) => void;
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

export function Timeline({ commits, mode, onSelectRepo }: Props) {
  const [selected, setSelected] = useState(0);
  const listRef = useRef<HTMLUListElement | null>(null);

  useEffect(() => {
    if (selected > commits.length - 1) setSelected(Math.max(0, commits.length - 1));
  }, [commits.length, selected]);

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
      }
    }
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [commits.length]);

  useEffect(() => {
    const row = listRef.current?.querySelector<HTMLLIElement>(
      `[data-row="${selected}"]`,
    );
    row?.scrollIntoView({ block: "nearest" });
  }, [selected]);

  if (commits.length === 0) {
    return <p className="panel-empty">No commits match.</p>;
  }

  const showRepo = mode === "all";

  return (
    <ul className={"timeline timeline-" + mode} ref={listRef}>
      {commits.map((c, i) => {
        const m = marker(c);
        return (
          <li
            key={`${c.repoPath}:${c.hash}`}
            data-row={i}
            className={"timeline-row" + (i === selected ? " selected" : "")}
            onClick={() => setSelected(i)}
          >
            <span className={"timeline-marker " + m.cls} title={m.title}>
              {m.glyph}
            </span>
            <span className="timeline-time">{timeAgo(c.timestamp)}</span>
            {showRepo && (
              <span
                className={
                  "timeline-repo" + (onSelectRepo ? " timeline-repo-clickable" : "")
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
            </span>
            <span className="timeline-author" title={c.email}>
              {c.author}
            </span>
          </li>
        );
      })}
    </ul>
  );
}
