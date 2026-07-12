import { useEffect, useState } from "react";
import { useLingui } from "@lingui/react/macro";

import { FILE_STATUS_BADGES } from "../lib/changedFileBadge";
import {
  changedFiles as fetchChangedFiles,
  openFileHistory,
} from "../lib/ipc";
import type { ChangedFile } from "../types";
import { diffMessages } from "../i18n/messages/diff";
import { translate } from "../i18n/translate";

interface Props {
  repoPath: string;
  hash: string;
  onOpenDiff?: (file: ChangedFile) => void;
}

export function ChangedFiles({ repoPath, hash, onOpenDiff }: Props) {
  const { i18n } = useLingui();
  const [files, setFiles] = useState<ChangedFile[] | null>(null);
  const [total, setTotal] = useState(0);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    setFiles(null);
    setTotal(0);
    setError(null);
    (async () => {
      try {
        const r = await fetchChangedFiles(repoPath, hash);
        if (!cancelled) {
          setFiles(r.files);
          setTotal(r.total);
        }
      } catch (e) {
        if (!cancelled) setError(String(e));
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [repoPath, hash]);

  if (error) {
    return (
      <div className="changed-files-empty">
        {translate(i18n, diffMessages.error, { error })}
      </div>
    );
  }
  if (files == null) {
    return (
      <div className="changed-files-empty">
        {i18n._(diffMessages.loadingFiles)}
      </div>
    );
  }
  if (files.length === 0) {
    return (
      <div className="changed-files-empty">
        {i18n._(diffMessages.noFileChanges)}
      </div>
    );
  }

  return (
    <div className="changed-files">
      {files.map((f, i) => {
        const badge = FILE_STATUS_BADGES[f.status] ?? FILE_STATUS_BADGES.modified;
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
            title={
              f.oldPath
                ? translate(i18n, diffMessages.renamedFrom, { path: f.oldPath })
                : f.path
            }
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
                <span
                  className="changed-file-bin"
                  title={i18n._(diffMessages.binaryFile)}
                >
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
            <button
              type="button"
              className="changed-file-history"
              title={translate(i18n, diffMessages.showHistory, { file: name })}
              aria-label={translate(i18n, diffMessages.showHistory, {
                file: name,
              })}
              // stopPropagation so opening history doesn't also open the diff
              // (the row click does that).
              onClick={(e) => {
                e.stopPropagation();
                void openFileHistory(repoPath, f.path);
              }}
            >
              🕘
            </button>
          </div>
        );
      })}
      {total > files.length && (
        <div className="changed-files-more">
          {translate(i18n, diffMessages.showingChangedFiles, {
            shown: files.length.toLocaleString(i18n.locale),
            total: total.toLocaleString(i18n.locale),
          })}
        </div>
      )}
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
