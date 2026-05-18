// Lightweight Shiki integration for the side-by-side diff.
//
// We deliberately use the singleton highlighter API and lazy-load both
// the engine and any new language on first sight. The highlighter is
// shared across all SideBySideDiff renders in a window.

import {
  createHighlighter,
  type BundledLanguage,
  type BundledTheme,
  type Highlighter,
} from "shiki";

// Keep this list focused on what dev tools repos actually contain.
// Adding a language here makes Shiki preload it on first creation; new
// languages can be loaded on-demand via highlighter.loadLanguage(...).
const LANGS: BundledLanguage[] = [
  "tsx",
  "ts",
  "jsx",
  "javascript",
  "json",
  "rust",
  "python",
  "go",
  "java",
  "c",
  "cpp",
  "csharp",
  "shell",
  "yaml",
  "toml",
  "html",
  "css",
  "scss",
  "markdown",
  "sql",
  "swift",
  "kotlin",
  "ruby",
  "php",
];

const THEMES: BundledTheme[] = ["github-light", "github-dark"];

const EXT_TO_LANG: Record<string, BundledLanguage> = {
  ts: "ts",
  tsx: "tsx",
  js: "javascript",
  jsx: "jsx",
  mjs: "javascript",
  cjs: "javascript",
  json: "json",
  rs: "rust",
  py: "python",
  go: "go",
  java: "java",
  c: "c",
  h: "c",
  cc: "cpp",
  cpp: "cpp",
  hpp: "cpp",
  cs: "csharp",
  sh: "shell",
  bash: "shell",
  zsh: "shell",
  yml: "yaml",
  yaml: "yaml",
  toml: "toml",
  html: "html",
  htm: "html",
  css: "css",
  scss: "scss",
  sass: "scss",
  md: "markdown",
  markdown: "markdown",
  sql: "sql",
  swift: "swift",
  kt: "kotlin",
  kts: "kotlin",
  rb: "ruby",
  php: "php",
};

export function langForPath(path: string): BundledLanguage | null {
  const i = path.lastIndexOf(".");
  if (i < 0) return null;
  const ext = path.slice(i + 1).toLowerCase();
  return EXT_TO_LANG[ext] ?? null;
}

let highlighterPromise: Promise<Highlighter> | null = null;

export function getHighlighter(): Promise<Highlighter> {
  if (!highlighterPromise) {
    highlighterPromise = createHighlighter({
      langs: LANGS,
      themes: THEMES,
    });
  }
  return highlighterPromise;
}

/**
 * Render a single source line into a styled HTML span sequence.
 * Returns null when the language isn't loaded or the highlighter isn't
 * ready yet — caller falls back to plain text.
 */
export function highlightLine(
  hl: Highlighter,
  text: string,
  lang: BundledLanguage,
  isDark: boolean,
): string | null {
  if (!hl.getLoadedLanguages().includes(lang)) return null;
  try {
    const html = hl.codeToHtml(text, {
      lang,
      theme: isDark ? "github-dark" : "github-light",
    });
    // Shiki wraps output in <pre class="shiki ..."><code>...lines...</code></pre>.
    // Strip the wrappers so we can inline into our row layout.
    const m = html.match(/<code[^>]*>([\s\S]*)<\/code>/);
    if (!m) return null;
    // Strip leading/trailing <span class="line"> wrappers — each line.
    return m[1].replace(/<span class="line">|<\/span>$/g, "").trim();
  } catch {
    return null;
  }
}
