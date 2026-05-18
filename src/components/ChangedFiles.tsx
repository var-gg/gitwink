import { useEffect, useState } from "react";

import { changedFiles as fetchChangedFiles } from "../lib/ipc";
import type { ChangedFile, ChangedFileStatus } from "../types";

interface Props {
  repoPath: string;
  hash: string;
  onOpenDiff?: (file: ChangedFile) => void;
}

const BADGES: Record<ChangedFileStatus, { label: string; cls: string }> = {
  new: { label: "NEW", cls: "badge-new" },
  modified: { label: "MOD", cls: "badge-mod" },
  renamed: { label: "REN", cls: "badge-ren" },
  deleted: { label: "DEL", cls: "badge-del" },
  copied: { label: "CP", cls: "badge-cp" },
  typechange: { label: "TYPE", cls: "badge-type" },
};

export function ChangedFiles({ repoPath, hash, onOpenDiff }: Props) {
  const [files, setFiles] = useState<ChangedFile[] | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    setFiles(null);
    setError(null);
    (async () => {
      try {
        const fs = await fetchChangedFiles(repoPath, hash);
        if (!cancelled) setFiles(fs);
      } catch (e) {
        if (!cancelled) setError(String(e));
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [repoPath, hash]);

  if (error) {
    return <div className="changed-files-empty">Error: {error}</div>;
  }
  if (files == null) {
    return <div className="changed-files-empty">Loading files…</div>;
  }
  if (files.length === 0) {
    return <div className="changed-files-empty">No file changes.</div>;
  }

  return (
    <div className="changed-files">
      {files.map((f, i) => {
        const badge = BADGES[f.status] ?? BADGES.modified;
        const slash = f.path.lastIndexOf("/");
        const dir = slash >= 0 ? f.path.slice(0, slash + 1) : "";
        const name = slash >= 0 ? f.path.slice(slash + 1) : f.path;
        return (
          <div
            key={`${f.path}:${i}`}
            className={
              "changed-file" + (onOpenDiff ? " changed-file-clickable" : "")
            }
            data-file-path={f.path}
            onClick={() => onOpenDiff?.(f)}
            title={f.oldPath ? `Renamed from ${f.oldPath}` : f.path}
          >
            <span className={"changed-file-badge " + badge.cls}>
              {badge.label}
            </span>
            <span className="changed-file-path">
              {f.oldPath && (
                <span className="changed-file-old">{f.oldPath} → </span>
              )}
              {dir && <span className="changed-file-dir">{dir}</span>}
              <span className="changed-file-name">{name}</span>
              {f.isBinary && (
                <span className="changed-file-bin" title="Binary file">
                  bin
                </span>
              )}
            </span>
            <span className="changed-file-stat">
              {f.isBinary ? (
                <span className="changed-file-size">
                  {formatSizeDelta(f.oldSize, f.newSize)}
                </span>
              ) : (
                <>
                  <span className="changed-file-plus">+{f.insertions}</span>
                  <span className="changed-file-minus">−{f.deletions}</span>
                </>
              )}
            </span>
          </div>
        );
      })}
    </div>
  );
}

function formatSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / 1024 / 1024).toFixed(2)} MB`;
}

function formatSizeDelta(
  oldSize: number | null,
  newSize: number | null,
): string {
  if (oldSize == null && newSize != null) return `+${formatSize(newSize)}`;
  if (newSize == null && oldSize != null) return `−${formatSize(oldSize)}`;
  if (oldSize != null && newSize != null) {
    return `${formatSize(oldSize)} → ${formatSize(newSize)}`;
  }
  return "—";
}
