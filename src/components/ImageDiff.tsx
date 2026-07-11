import { useEffect, useState } from "react";
import { useLingui } from "@lingui/react/macro";

import { commitFileBlobs } from "../lib/ipc";
import type { CommitFileBlobs } from "../types";
import { diffMessages } from "../i18n/messages/diff";
import { translate } from "../i18n/translate";

interface Props {
  repoPath: string;
  hash: string;
  filePath: string;
  oldPath: string | null;
  oldSize: number | null;
  newSize: number | null;
}

const MIME: Record<string, string> = {
  png: "image/png",
  jpg: "image/jpeg",
  jpeg: "image/jpeg",
  gif: "image/gif",
  webp: "image/webp",
  svg: "image/svg+xml",
  bmp: "image/bmp",
  ico: "image/x-icon",
};

function formatSize(bytes: number | null): string {
  if (bytes == null) return "—";
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / 1024 / 1024).toFixed(2)} MB`;
}

export function ImageDiff({
  repoPath,
  hash,
  filePath,
  oldPath,
  oldSize,
  newSize,
}: Props) {
  const { i18n } = useLingui();
  const [blobs, setBlobs] = useState<CommitFileBlobs | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    setBlobs(null);
    setError(null);
    (async () => {
      try {
        const b = await commitFileBlobs(repoPath, hash, filePath, oldPath);
        if (!cancelled) setBlobs(b);
      } catch (e) {
        if (!cancelled) setError(String(e));
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [repoPath, hash, filePath, oldPath]);

  if (error) {
    return (
      <div className="image-diff-error">
        {translate(i18n, diffMessages.error, { error })}
      </div>
    );
  }
  if (!blobs) {
    return (
      <div className="image-diff-loading">
        {i18n._(diffMessages.loadingImage)}
      </div>
    );
  }

  if (blobs.isLfs) {
    return (
      <div className="binary-info">
        <div className="binary-info-title">{i18n._(diffMessages.lfsFile)}</div>
        <div className="binary-info-meta">
          {formatSize(oldSize)} → {formatSize(newSize)}
        </div>
        <div className="binary-info-hint">
          {i18n._(diffMessages.lfsHint)}
        </div>
      </div>
    );
  }

  const mime = MIME[blobs.extension] ?? "application/octet-stream";
  const oldSrc = blobs.oldBase64
    ? `data:${mime};base64,${blobs.oldBase64}`
    : null;
  const newSrc = blobs.newBase64
    ? `data:${mime};base64,${blobs.newBase64}`
    : null;

  return (
    <div className="image-diff">
      <div className="image-diff-side">
        <div className="image-diff-label">
          {i18n._(diffMessages.before)} · {formatSize(oldSize)}
        </div>
        <div className="image-diff-frame image-diff-frame-old">
          {oldSrc ? (
            <img src={oldSrc} alt={i18n._(diffMessages.beforeAlt)} />
          ) : (
            <div className="image-diff-empty">
              {i18n._(diffMessages.fileDidNotExist)}
            </div>
          )}
        </div>
      </div>
      <div className="image-diff-side">
        <div className="image-diff-label">
          {i18n._(diffMessages.after)} · {formatSize(newSize)}
        </div>
        <div className="image-diff-frame image-diff-frame-new">
          {newSrc ? (
            <img src={newSrc} alt={i18n._(diffMessages.afterAlt)} />
          ) : (
            <div className="image-diff-empty">
              {i18n._(diffMessages.deleted)}
            </div>
          )}
        </div>
      </div>
    </div>
  );
}
