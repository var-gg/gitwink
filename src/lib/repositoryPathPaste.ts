export const MAX_PASTED_REPOSITORY_PATHS = 32;

const MAX_CLIPBOARD_PATH_TEXT_LENGTH = 64 * 1024;

function splitQuotedPathLine(line: string): string[] {
  const trimmed = line.trim();
  if (trimmed[0] !== '"' && trimmed[0] !== "'") return [trimmed];

  const paths: string[] = [];
  let cursor = 0;
  while (cursor < trimmed.length) {
    while (/\s/.test(trimmed[cursor] ?? "")) cursor += 1;
    if (cursor >= trimmed.length) break;

    const quote = trimmed[cursor];
    if (quote !== '"' && quote !== "'") return [trimmed];
    const closing = trimmed.indexOf(quote, cursor + 1);
    if (closing < 0) return [trimmed];

    paths.push(trimmed.slice(cursor + 1, closing));
    cursor = closing + 1;
    if (cursor < trimmed.length && !/\s/.test(trimmed[cursor])) {
      return [trimmed];
    }
  }

  return paths.length > 0 ? paths : [trimmed];
}

function stripBalancedQuotes(value: string): string {
  const first = value[0];
  const last = value[value.length - 1];
  if (value.length >= 2 && first === last && (first === '"' || first === "'")) {
    return value.slice(1, -1).trim();
  }
  return value;
}

function fileUrlToPath(value: string): string | null {
  if (!/^file:/i.test(value)) return value;
  try {
    const url = new URL(value);
    if (url.protocol !== "file:") return null;

    let pathname = decodeURIComponent(url.pathname);
    const localHost = !url.hostname || url.hostname.toLowerCase() === "localhost";
    if (localHost && /^\/[a-zA-Z]:[\\/]/.test(pathname)) {
      pathname = pathname.slice(1);
    }
    if (!localHost) return `//${url.hostname}${pathname}`;
    return pathname;
  } catch {
    return null;
  }
}

function isAbsolutePath(value: string): boolean {
  return (
    /^[a-zA-Z]:[\\/]/.test(value) ||
    /^\\\\/.test(value) ||
    /^\//.test(value)
  );
}

function normalizePathCandidate(raw: string): string | null {
  if (raw.includes("\0")) return null;
  const unquoted = stripBalancedQuotes(raw.trim());
  if (!unquoted) return null;
  const path = fileUrlToPath(unquoted);
  if (!path || !isAbsolutePath(path)) return null;
  return path;
}

function pathIdentity(path: string): string {
  if (/^[a-zA-Z]:[\\/]/.test(path) || /^\\\\/.test(path) || /^\/\/[^/]/.test(path)) {
    return path.replace(/\//g, "\\").toLowerCase();
  }
  return path;
}

/** Parses the plain-text formats produced by Windows Explorer's Copy as path,
 * Finder/Linux file URLs, terminals, and multi-selection clipboard payloads.
 * Validation that the path is actually a Git worktree remains in Rust. */
export function parseRepositoryPathClipboard(text: string): string[] {
  if (text.length > MAX_CLIPBOARD_PATH_TEXT_LENGTH) return [];
  const seen = new Set<string>();
  const paths: string[] = [];

  for (const line of text.split(/\r?\n/)) {
    if (!line.trim()) continue;
    for (const raw of splitQuotedPathLine(line)) {
      const path = normalizePathCandidate(raw);
      // A paste payload must consist entirely of paths. This fail-closed rule
      // keeps copied source, logs, or prose containing one path-looking line
      // from registering a repository unexpectedly.
      if (!path) return [];
      const identity = pathIdentity(path);
      if (seen.has(identity)) continue;
      seen.add(identity);
      paths.push(path);
      if (paths.length >= MAX_PASTED_REPOSITORY_PATHS) return paths;
    }
  }

  return paths;
}

export function repositoryPasteShortcut(platformHint?: string): string {
  const platform =
    platformHint ?? (typeof navigator === "undefined" ? "" : navigator.platform);
  return /mac|iphone|ipad|ipod/i.test(platform) ? "⌘V" : "Ctrl+V";
}
