import { useLingui } from "@lingui/react/macro";
import type { ChangedFile } from "../types";
import { openFileHistory } from "../lib/ipc";
import { FILE_STATUS_BADGES } from "../lib/changedFileBadge";
import { diffMessages } from "../i18n/messages/diff";
import { translate } from "../i18n/translate";

function formatSize(bytes: number | null): string {
  if (bytes == null) return "—";
  if (bytes < 1024) return `${bytes} B`;
  return bytes < 1048576 ? `${(bytes / 1024).toFixed(1)} KB` : `${(bytes / 1048576).toFixed(2)} MB`;
}

export function DiffFileList({ files, selectedPath, onSelect, repoPath }: {
  files: ChangedFile[]; selectedPath: string | null; onSelect: (path: string) => void; repoPath: string;
}) {
  const { i18n } = useLingui();
  return <>{files.map((f) => {
              const isSel = f.path === selectedPath;
              const slash = f.path.lastIndexOf("/");
              const dir = slash >= 0 ? f.path.slice(0, slash + 1) : "";
              const name = slash >= 0 ? f.path.slice(slash + 1) : f.path;
              const badge =
                FILE_STATUS_BADGES[f.status] ?? FILE_STATUS_BADGES.modified;
              return (
                <div
                  key={f.path}
                  className={"diff-file-wrap" + (isSel ? " active" : "")}
                  data-file-path={f.path}
                >
                  <button
                    className={"diff-file" + (isSel ? " active" : "")}
                    onClick={() => onSelect(f.path)}
                    title={
                      f.oldPath
                        ? translate(i18n, diffMessages.renamedFrom, {
                            path: f.oldPath,
                          })
                        : f.path
                    }
                  >
                    <div className="diff-file-line">
                      <span className={"changed-file-badge " + badge.cls}>
                        {badge.label}
                      </span>
                      <span className="diff-file-name">{name}</span>
                      {f.isBinary && (
                        <span
                          className="changed-file-bin"
                          title={i18n._(diffMessages.binaryFile)}
                        >
                          bin
                        </span>
                      )}
                      <span className="diff-file-stat">
                        {f.isBinary ? (
                          <span className="diff-file-binsize">
                            {formatSize(f.newSize ?? f.oldSize)}
                          </span>
                        ) : (
                          <>
                            <span className="changed-file-plus">
                              +{f.insertions}
                            </span>
                            <span className="changed-file-minus">
                              −{f.deletions}
                            </span>
                          </>
                        )}
                      </span>
                    </div>
                    {(dir || f.oldPath) && (
                      <div className="diff-file-dir">
                        {f.oldPath && (
                          <span className="changed-file-old">
                            {f.oldPath} →{" "}
                          </span>
                        )}
                        {dir}
                      </div>
                    )}
                  </button>
                  <button
                    className="diff-file-history"
                    title={i18n._(diffMessages.showHistoryInPanel)}
                    aria-label={translate(i18n, diffMessages.showHistory, {
                      file: name,
                    })}
                    onClick={() => void openFileHistory(repoPath, f.path)}
                  >
                    🕘
                  </button>
                </div>
              );
            })}</>;
}
