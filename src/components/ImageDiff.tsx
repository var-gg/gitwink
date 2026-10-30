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

  const mime = MIME[blobs.extension] ?? "application/octet-stream";
  const emptyLabel = (side: "old" | "new") => {
    switch (blobs[side].status) {
      case "absent": return i18n._(side === "old" ? diffMessages.fileDidNotExist : diffMessages.deleted);
      case "too_large": return i18n._(diffMessages.imageTooLarge);
      case "lfs_missing": return i18n._(diffMessages.lfsHint);
      default: return i18n._(diffMessages.imageUnreadable);
    }
  };
  return <div className="image-diff">
    {(["old", "new"] as const).map(side => <div className="image-diff-side" key={side}>
      <div className="image-diff-label">
        {i18n._(side === "old" ? diffMessages.before : diffMessages.after)} · {formatSize(blobs[side].size ?? (side === "old" ? oldSize : newSize))}
      </div>
      <div className={`image-diff-frame image-diff-frame-${side}`}>
        {blobs[side].status === "ready" ? <img src={`data:${mime};base64,${blobs[side].base64}`} alt={i18n._(side === "old" ? diffMessages.beforeAlt : diffMessages.afterAlt)} />
          : <div className="image-diff-empty">{emptyLabel(side)}</div>}
      </div>
    </div>)}
  </div>;
}
