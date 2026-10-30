import { useState } from "react";
import { useLingui } from "@lingui/react";
import { reviewMessages as m } from "../../i18n/messages/review";
import { diffMessages } from "../../i18n/messages/diff";
import { translate } from "../../i18n/translate";
import type { ReviewFile } from "../../lib/review/ipc";
import { FILE_STATUS_BADGES } from "../../lib/changedFileBadge";

export function ReviewFiles({ files, active, onOpen, selected, onSelect }: {
  files: ReviewFile[]; active: string | null; onOpen: (id: string) => void;
  selected: Set<string>; onSelect: (ids: Set<string>) => void;
}) {
  const { i18n } = useLingui();
  const [query, setQuery] = useState("");
  const [limit, setLimit] = useState(300);
  const visible = files.filter(f => `${f.path} ${f.oldPath ?? ""}`.toLowerCase().includes(query.toLowerCase()));
  const groups = { range: m.changes, staged: m.staged, unstaged: m.unstaged, untracked: m.untracked };
  return <aside className="review-files">
    <div className="review-file-tools">
      <input aria-label={i18n._(m.filter)} placeholder={i18n._(m.filter)} value={query} onChange={e => setQuery(e.target.value)} />
      <div><button onClick={() => onSelect(new Set(files.map(f => f.id)))}>{i18n._(m.all)}</button>
        <button onClick={() => onSelect(new Set())}>{i18n._(m.none)}</button></div>
      <small>{translate(i18n, m.selected, { selected: selected.size, total: files.length })}</small>
    </div>
    <div className="review-file-scroll">
      {Object.entries(groups).map(([group, label]) => {
        const groupFiles = visible.filter(f => f.group === group);
        if (!groupFiles.length) return null;
        return <section key={group}><h2>{i18n._(label)} <span>{groupFiles.length}</span></h2>
          {groupFiles.slice(0, limit).map(f => {
            const badge = FILE_STATUS_BADGES[f.status] ?? FILE_STATUS_BADGES.modified;
            return <div key={f.id} className={`review-file-row${active === f.id ? " active" : ""}`}>
              <input type="checkbox" aria-label={f.path} checked={selected.has(f.id)} onChange={e => {
                const next = new Set(selected); if (e.target.checked) next.add(f.id); else next.delete(f.id); onSelect(next);
              }} />
              <button onClick={() => onOpen(f.id)} title={f.oldPath ? `${f.oldPath} → ${f.path}` : f.path}>
                <span className={`changed-file-badge ${badge.cls}`}>{badge.label}</span>
                <span className="review-file-path">{f.path}{f.oldPath && <small>← {f.oldPath}</small>}</span>
                <small>{f.isBinary ? "bin" : `+${f.insertions} −${f.deletions}`}</small>
              </button>
            </div>;
          })}
          {groupFiles.length > limit && <button className="review-more" onClick={() => setLimit(n => n + 300)}>
            {translate(i18n, diffMessages.showingChangedFiles, { shown: limit, total: groupFiles.length })} · {i18n._(m.more)}
          </button>}
        </section>;
      })}
      {!visible.length && <p>{i18n._(files.length ? m.noMatch : m.noChanges)}</p>}
    </div>
  </aside>;
}
