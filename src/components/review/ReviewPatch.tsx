import { useEffect, useState } from "react";
import { useLingui } from "@lingui/react";
import { diffMessages } from "../../i18n/messages/diff";
import { reviewMessages as m } from "../../i18n/messages/review";
import { reviewPatch, type ReviewFile, type ReviewSnapshot } from "../../lib/review/ipc";
import type { FilePatch } from "../../types";
import { SideBySideDiff } from "../SideBySideDiff";

export function ReviewPatch({ snapshot, file }: { snapshot: ReviewSnapshot; file: ReviewFile | undefined }) {
  const { i18n } = useLingui();
  const [context, setContext] = useState(3);
  const [patch, setPatch] = useState<FilePatch | null>(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    let cancelled = false;
    setPatch(null); setError(null);
    if (file && !file.isBinary) {
      void reviewPatch(snapshot.id, file.id, context).then(p => { if (!cancelled) setPatch(p); })
        .catch(e => { if (!cancelled) setError(String(e)); });
    }
    return () => { cancelled = true; };
  }, [snapshot.id, file?.id, context]);
  if (!file) return <main className="review-empty">{i18n._(m.choose)}</main>;
  return <main className="review-patch">
    <div className="review-patch-title"><strong>{file.path}</strong>
      {!file.isBinary && <div role="group" aria-label={i18n._(diffMessages.context)}>{[3, 25].map(n =>
        <button key={n} className={n === context ? "active" : ""} aria-pressed={n === context} onClick={() => setContext(n)}>±{n}</button>)}</div>}
    </div>
    {file.isBinary ? <div className="review-empty">{i18n._(m.binary)}<p>{file.oldSize ?? "—"} → {file.newSize ?? "—"} B</p></div>
      : error ? <div className="review-error" role="alert">{i18n._(m.failed)}<pre>{error}</pre></div>
      : !patch ? <div className="review-empty" role="status">{i18n._(diffMessages.loading)}</div>
      : <><div className="review-provenance" title={`${patch.oldRevision}:${patch.oldPath} → ${patch.newRevision}:${patch.newPath}`}>
          <code>{patch.oldRevision}</code> → <code>{patch.newRevision}</code>
        </div>
        {patch.truncated && <div className="diff-limit-notice">{i18n._(diffMessages.patchTruncated)}</div>}
        <div className="review-diff-content"><SideBySideDiff text={patch.text} filePath={file.path} fileKey={`${snapshot.id}:${file.id}`} locked /></div>
      </>}
  </main>;
}
