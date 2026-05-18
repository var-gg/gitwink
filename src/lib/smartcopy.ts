// Smart-copy reference formats. Frontend-only — repo origin (for GitHub URL
// inference) is left to a future version.

export function refLine(repoName: string, shortHash: string): string {
  return `# gitwink ref: ${repoName} @ ${shortHash}`;
}

export function refLineWithFile(
  repoName: string,
  shortHash: string,
  filePath: string,
  start: number | null,
  end: number | null,
): string {
  const lines =
    start != null && end != null && start !== end
      ? ` L${start}-L${end}`
      : start != null
        ? ` L${start}`
        : "";
  return `# gitwink ref: ${repoName} @ ${shortHash}:${filePath}${lines}`;
}

/**
 * Extract a line range from the current text selection within a
 * SideBySideDiff. Returns null if the selection isn't inside diff lines
 * (or doesn't exist).
 */
export function getDiffSelectionRange(): {
  start: number;
  end: number;
  side: string;
} | null {
  const sel = window.getSelection();
  if (!sel || sel.rangeCount === 0 || sel.isCollapsed) return null;
  const range = sel.getRangeAt(0);

  function lineEl(node: Node): HTMLElement | null {
    const el =
      node.nodeType === Node.TEXT_NODE
        ? (node.parentElement as HTMLElement | null)
        : (node as HTMLElement);
    return el?.closest<HTMLElement>("[data-line-num]") ?? null;
  }

  const startEl = lineEl(range.startContainer);
  const endEl = lineEl(range.endContainer);
  if (!startEl || !endEl) return null;
  const s = parseInt(startEl.dataset.lineNum ?? "0", 10);
  const e = parseInt(endEl.dataset.lineNum ?? "0", 10);
  return {
    start: Math.min(s, e),
    end: Math.max(s, e),
    side: startEl.dataset.side ?? "left",
  };
}
