import { describe, expect, it } from "vitest";

import {
  MAX_PASTED_REPOSITORY_PATHS,
  parseRepositoryPathClipboard,
  repositoryPasteShortcut,
} from "./repositoryPathPaste";

describe("repository path clipboard parsing", () => {
  it("accepts Explorer Copy as path quotes and preserves spaces", () => {
    expect(
      parseRepositoryPathClipboard('  "C:\\Users\\Ada\\My Repo"  '),
    ).toEqual(["C:\\Users\\Ada\\My Repo"]);
  });

  it("accepts multiple quoted paths and deduplicates Windows identities", () => {
    expect(
      parseRepositoryPathClipboard(
        '"C:\\repos\\one" "D:\\repos\\two"\r\n"c:\\REPOS\\ONE"',
      ),
    ).toEqual(["C:\\repos\\one", "D:\\repos\\two"]);
  });

  it("accepts UNC, POSIX, and file URL path forms", () => {
    expect(
      parseRepositoryPathClipboard(
        [
          "\\\\server\\share\\repo",
          "/Users/ada/repo",
          "file:///C:/Users/Ada/repo%20one",
          "file://nas/share/repo-two",
        ].join("\n"),
      ),
    ).toEqual([
      "\\\\server\\share\\repo",
      "/Users/ada/repo",
      "C:/Users/Ada/repo one",
      "//nas/share/repo-two",
    ]);
  });

  it("ignores prose, relative paths, web URLs, malformed URLs, and NULs", () => {
    expect(
      parseRepositoryPathClipboard(
        [
          "copy this repository",
          "relative/repo",
          "~/relative/repo",
          "https://example.com/repo",
          "file:///%E0%A4%A",
          "C:\\repo\0hidden",
        ].join("\n"),
      ),
    ).toEqual([]);
  });

  it("rejects a mixed document even when one line looks like a path", () => {
    expect(
      parseRepositoryPathClipboard("stack trace follows\nC:\\repos\\one"),
    ).toEqual([]);
  });

  it("rejects oversized non-path clipboard payloads instead of truncating", () => {
    expect(
      parseRepositoryPathClipboard(`/repo\n${"x".repeat(64 * 1024)}`),
    ).toEqual([]);
  });

  it("bounds a pathological multi-path paste", () => {
    const text = Array.from(
      { length: MAX_PASTED_REPOSITORY_PATHS + 10 },
      (_, index) => `/repos/${index}`,
    ).join("\n");
    expect(parseRepositoryPathClipboard(text)).toHaveLength(
      MAX_PASTED_REPOSITORY_PATHS,
    );
  });
});

describe("repository paste shortcut", () => {
  it("uses the native modifier on Apple platforms", () => {
    expect(repositoryPasteShortcut("MacIntel")).toBe("⌘V");
    expect(repositoryPasteShortcut("iPad")).toBe("⌘V");
  });

  it("uses Ctrl everywhere else", () => {
    expect(repositoryPasteShortcut("Win32")).toBe("Ctrl+V");
    expect(repositoryPasteShortcut("Linux x86_64")).toBe("Ctrl+V");
  });
});
