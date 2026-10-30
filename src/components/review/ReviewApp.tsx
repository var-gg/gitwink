import { useEffect, useRef, useState } from "react";
import { useLingui } from "@lingui/react";
import { listen } from "@tauri-apps/api/event";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { reviewMessages as m } from "../../i18n/messages/review";
import { diffMessages } from "../../i18n/messages/diff";
import { translate } from "../../i18n/translate";
import { markReviewed, reviewCopyPatches, reviewSnapshot, takePendingReview, validateReview,
  type ReviewRequest, type ReviewSnapshot } from "../../lib/review/ipc";
import { buildReviewContext } from "../../lib/review/context";
import { initialReviewSelection } from "../../lib/review/selection";
import { ReviewFiles } from "./ReviewFiles";
import { ReviewPatch } from "./ReviewPatch";
import "./review.css";
import "../diffFeedback.css";

export function ReviewApp() {
  const { i18n } = useLingui();
  const [request, setRequest] = useState<ReviewRequest | null>(null);
  const [snapshot, setSnapshot] = useState<ReviewSnapshot | null>(null);
  const [nonce, setNonce] = useState(0);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [selected, setSelected] = useState(new Set<string>());
  const [active, setActive] = useState<string | null>(null);
  const [base, setBase] = useState("HEAD~1");
  const [head, setHead] = useState("HEAD");
  const generation = useRef(0);

  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | undefined;
    let draining = false;
    let again = false;
    const drain = async () => {
      if (draining) { again = true; return; }
      draining = true;
      try {
        do {
          again = false;
          const pending = await takePendingReview();
          if (!cancelled && pending) {
            setRequest(pending); setNotice(null);
            if (pending.kind === "range") { setBase(pending.base); setHead(pending.head); }
          }
        } while (again && !cancelled);
      } catch (e) { if (!cancelled) setError(String(e)); }
      finally { draining = false; }
    };
    void listen("review://pending", () => void drain()).then(un => {
      if (cancelled) { un(); return; }
      unlisten = un; void drain();
    }).catch(e => { if (!cancelled) setError(String(e)); });
    return () => { cancelled = true; unlisten?.(); };
  }, []);

  useEffect(() => {
    const id = ++generation.current;
    setSnapshot(null); setError(null); setActive(null); setSelected(new Set()); setBusy(false);
    if (!request) return;
    void reviewSnapshot(request).then(s => {
      if (id !== generation.current) return;
      const initial = initialReviewSelection(s.files);
      setSnapshot(s); setSelected(initial.selected); setActive(initial.active);
    }).catch(e => { if (id === generation.current) setError(String(e)); });
    return () => { generation.current++; };
  }, [request, nonce]);

  async function copy() {
    if (!snapshot) return;
    const id = generation.current;
    setBusy(true); setNotice(null); setError(null);
    try {
      await validateReview(snapshot.id);
      const ids = snapshot.files.filter(f => selected.has(f.id) && !f.isBinary && f.insertions + f.deletions <= 1200).slice(0, 32).map(f => f.id);
      const patches = await reviewCopyPatches(snapshot.id, ids);
      const text = await buildReviewContext(snapshot, selected, async file => {
        if (!patches[file]) throw new Error("Selected patch is unavailable. Refresh the review.");
        return patches[file];
      });
      await validateReview(snapshot.id);
      if (id !== generation.current) return;
      await writeText(text);
      setNotice(i18n._(m.copied));
    } catch (e) { if (id === generation.current) setError(String(e)); }
    finally { if (id === generation.current) setBusy(false); }
  }
  async function mark() {
    if (!snapshot) return;
    const id = generation.current;
    setBusy(true); setError(null);
    try {
      await markReviewed(snapshot.id);
      if (id !== generation.current) return;
      setNotice(i18n._(m.saved));
      setRequest({ kind: "sinceReviewed", repoPath: snapshot.repoPath });
    } catch (e) { if (id === generation.current) setError(String(e)); }
    finally { if (id === generation.current) setBusy(false); }
  }
  const modes = { sinceReviewed: m.since, range: m.range, worktree: m.worktree };
  return <div className="review-shell">
    <header className="review-header">
      <div><span className="review-eyebrow">gitwink / {i18n._(m.title)}</span>
        <h1>{snapshot?.repoName ?? request?.repoPath.split(/[\\/]/).pop() ?? "gitwink"}</h1>
        {snapshot && <div className="review-branch">{snapshot.branch.replace(/^refs\/heads\//, "")}</div>}
      </div>
      <div className="review-actions">
        <button onClick={() => { setNotice(null); setNonce(n => n + 1); }} disabled={!request || busy}>{i18n._(m.refresh)}</button>
        <button onClick={() => void copy()} disabled={!snapshot || !selected.size || busy}>{i18n._(m.copy)}</button>
        {snapshot?.canMarkReviewed && <button className="review-primary" onClick={() => void mark()} disabled={busy}>
          {i18n._(snapshot.baselineStatus === "missing" && snapshot.mode === "sinceReviewed" ? m.start : m.mark)}</button>}
      </div>
    </header>
    <nav className="review-tabs" aria-label={i18n._(m.title)}>
      {Object.entries(modes).map(([kind, label]) => <button key={kind} disabled={!request || busy}
        aria-pressed={request?.kind === kind} className={request?.kind === kind ? "active" : ""}
        onClick={() => { if (request) { setNotice(null); setRequest(kind === "range"
          ? { kind, repoPath: request.repoPath, base, head } : { kind: kind as "sinceReviewed" | "worktree", repoPath: request.repoPath }); } }}>
        {i18n._(label)}</button>)}
    </nav>
    {request?.kind === "range" && <form className="review-range" onSubmit={e => { e.preventDefault(); setRequest({ ...request, base, head }); }}>
      <label>{i18n._(m.base)}<input value={base} onChange={e => setBase(e.target.value)} required maxLength={512} /></label>
      <span>→</span><label>{i18n._(m.head)}<input value={head} onChange={e => setHead(e.target.value)} required maxLength={512} /></label>
      <button disabled={busy}>{i18n._(m.compare)}</button>
    </form>}
    {snapshot?.mode === "sinceReviewed" && snapshot.baselineStatus !== "ready" &&
      <div className="review-notice">{i18n._(snapshot.baselineStatus === "missing" ? m.missing : m.diverged)}</div>}
    {notice && <div className="review-notice" role="status">{notice}</div>}
    {error && <div className="review-error" role="alert">{i18n._(m.failed)}<pre>{error}</pre></div>}
    <div className="review-body">{snapshot ? <>
      <ReviewFiles files={snapshot.files} active={active} onOpen={setActive} selected={selected} onSelect={setSelected} />
      <ReviewPatch key={`${snapshot.id}:${active}`} snapshot={snapshot} file={snapshot.files.find(f => f.id === active)} />
    </> : !error && <div className="review-empty" role="status">{i18n._(diffMessages.loading)}</div>}</div>
    <footer className="review-footer"><span>{i18n._(m.readonly)}</span>
      {snapshot && <span>{translate(i18n, m.snapshot, { time: new Date(snapshot.createdAt * 1000).toLocaleTimeString(i18n.locale) })}</span>}
    </footer>
  </div>;
}
