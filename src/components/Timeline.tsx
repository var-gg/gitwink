import { useEffect, useRef, useState } from "react";

import type { CommitSummary } from "../types";

interface Props {
  commits: CommitSummary[];
}

function timeAgo(unixSeconds: number): string {
  const now = Math.floor(Date.now() / 1000);
  const diff = Math.max(0, now - unixSeconds);
  if (diff < 60) return `${diff}s`;
  if (diff < 3600) return `${Math.floor(diff / 60)}m`;
  if (diff < 86_400) return `${Math.floor(diff / 3600)}h`;
  return `${Math.floor(diff / 86_400)}d`;
}

export function Timeline({ commits }: Props) {
  const [selected, setSelected] = useState(0);
  const listRef = useRef<HTMLUListElement | null>(null);

  useEffect(() => {
    if (selected > commits.length - 1) setSelected(Math.max(0, commits.length - 1));
  }, [commits.length, selected]);

  useEffect(() => {
    function onKey(e: KeyboardEvent) {
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
    return <p className="panel-empty">No commits in the last 7 days.</p>;
  }

  return (
    <ul className="timeline" ref={listRef}>
      {commits.map((c, i) => (
        <li
          key={`${c.repoPath}:${c.hash}`}
          data-row={i}
          className={"timeline-row" + (i === selected ? " selected" : "")}
          onClick={() => setSelected(i)}
        >
          <span className="timeline-time">{timeAgo(c.timestamp)}</span>
          <span className="timeline-repo" title={c.repoPath}>
            {c.repoName}
          </span>
          <span className="timeline-summary" title={c.summary}>
            {c.summary}
          </span>
          <span className="timeline-author" title={c.email}>
            {c.author}
          </span>
        </li>
      ))}
    </ul>
  );
}
