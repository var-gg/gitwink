import { msg } from "@lingui/core/macro";

export const diffMessages = {
  patchTruncated: msg({ id: "diff.patch.truncated", message: "Patch exceeds 5 MiB. Only complete lines within the limit are shown; copied patches also identify omissions." }),
  imageTooLarge: msg({ id: "diff.image.tooLarge", message: "Image exists, but exceeds the 4 MiB preview limit." }),
  imageUnreadable: msg({ id: "diff.image.unreadable", message: "Image exists, but its content could not be read." }),
  error: msg({ id: "diff.error", message: "Error: {error}" }),
  loadingFiles: msg({ id: "diff.files.loading", message: "Loading files…" }),
  noFileChanges: msg({
    id: "diff.files.noChanges",
    message: "No file changes.",
  }),
  renamedFrom: msg({
    id: "diff.files.renamedFrom",
    message: "Renamed from {path}",
  }),
  binaryFile: msg({ id: "diff.binaryFile", message: "Binary file" }),
  showHistory: msg({
    id: "diff.files.showHistory",
    message: "Show history of {file}",
  }),
  showHistoryInPanel: msg({
    id: "diff.files.showHistoryInPanel",
    message: "Show this file's history in the panel",
  }),
  showingChangedFiles: msg({
    id: "diff.files.showing",
    message: "Showing {shown} of {total} changed files",
  }),
  filesLoadFailed: msg({
    id: "diff.files.loadFailed",
    message: "Couldn't load this commit's files.",
  }),
  filesLoadFailedTitle: msg({
    id: "diff.files.loadFailedTitle",
    message: "Couldn't load this commit's files",
  }),

  copy: msg({ id: "clipboard.copy", message: "Copy" }),
  copyWithReference: msg({
    id: "clipboard.copyWithReference",
    message: "Copy with reference",
  }),
  copyFileDiff: msg({
    id: "clipboard.copyFileDiff",
    message: "Copy file diff",
  }),
  copyShortHash: msg({
    id: "clipboard.copyShortHash",
    message: "Copy short hash",
  }),
  copyFullHash: msg({
    id: "clipboard.copyFullHash",
    message: "Copy full hash",
  }),
  showFileHistory: msg({
    id: "clipboard.showFileHistory",
    message: "Show file history",
  }),
  showFileHistoryDecorated: msg({
    id: "clipboard.showFileHistoryDecorated",
    message: "🕘 Show file history",
  }),
  copyFilePath: msg({
    id: "clipboard.copyFilePath",
    message: "Copy file path",
  }),
  copyAiContext: msg({
    id: "clipboard.copyAiContext",
    message: "Copy as AI context",
  }),
  copyCommitMessage: msg({
    id: "clipboard.copyCommitMessage",
    message: "Copy commit message",
  }),

  waitingForFile: msg({
    id: "diff.waitingForFile",
    message: "Waiting for a file…",
  }),
  context: msg({ id: "diff.context", message: "Diff context" }),
  contextDefault: msg({
    id: "diff.context.default",
    message: "Default—3 lines of context around each change",
  }),
  contextExpanded: msg({
    id: "diff.context.expanded",
    message: "Expanded—25 lines of context",
  }),
  contextFull: msg({ id: "diff.context.full", message: "Full" }),
  contextFullTitle: msg({
    id: "diff.context.fullTitle",
    message: "Whole file, with changes highlighted",
  }),
  contextTooLarge: msg({
    id: "diff.context.tooLarge",
    message: "File too large for full context",
  }),
  sync: msg({ id: "diff.sync", message: "Sync" }),
  syncOn: msg({
    id: "diff.sync.on",
    message:
      "Scroll sync on—old and new move together. Select to unlock independent scrolling.",
  }),
  syncOff: msg({
    id: "diff.sync.off",
    message:
      "Scroll sync off—sides scroll independently. Select to lock them together.",
  }),
  pickFile: msg({ id: "diff.pickFile", message: "Pick a file." }),
  cannotShow: msg({ id: "diff.cannotShow", message: "The diff can't be shown." }),
  binaryUnsupported: msg({
    id: "diff.binaryUnsupported",
    message: "gitwink doesn't render diffs for non-image binaries yet.",
  }),
  loadFailed: msg({
    id: "diff.loadFailed",
    message: "Couldn't load this diff",
  }),
  loading: msg({ id: "diff.loading", message: "Loading diff…" }),
  loadingShort: msg({ id: "common.loading", message: "Loading…" }),

  loadingImage: msg({ id: "diff.image.loading", message: "Loading image…" }),
  lfsFile: msg({ id: "diff.image.lfsFile", message: "Git LFS file" }),
  lfsHint: msg({
    id: "diff.image.lfsHint",
    message:
      "The actual image is stored in Git LFS. gitwink doesn't fetch LFS objects yet, so there's nothing to preview.",
  }),
  before: msg({ id: "diff.image.before", message: "Before" }),
  after: msg({ id: "diff.image.after", message: "After" }),
  beforeAlt: msg({ id: "diff.image.beforeAlt", message: "before" }),
  afterAlt: msg({ id: "diff.image.afterAlt", message: "after" }),
  fileDidNotExist: msg({
    id: "diff.image.fileDidNotExist",
    message: "(file did not exist)",
  }),
  deleted: msg({ id: "diff.image.deleted", message: "(deleted)" }),

  overview: msg({
    id: "diff.minimap.overview",
    message: "Diff overview—click or drag to jump to a change",
  }),
  oldViewport: msg({
    id: "diff.minimap.oldViewport",
    message: "Old (before) viewport",
  }),
  newViewport: msg({
    id: "diff.minimap.newViewport",
    message: "New (after) viewport",
  }),
  realign: msg({
    id: "diff.minimap.realign",
    message: "Re-align the two sides",
  }),

  noText: msg({ id: "diff.text.none", message: "No textual diff." }),
  find: msg({ id: "diff.find.placeholder", message: "Find in diff…" }),
  findAria: msg({ id: "diff.find.aria", message: "Find in diff" }),
  previousMatch: msg({
    id: "diff.find.previous",
    message: "Previous match",
  }),
  previousMatchTitle: msg({
    id: "diff.find.previousTitle",
    message: "Previous match (Shift+Enter)",
  }),
  nextMatch: msg({ id: "diff.find.next", message: "Next match" }),
  nextMatchTitle: msg({
    id: "diff.find.nextTitle",
    message: "Next match (Enter)",
  }),
  closeFind: msg({ id: "diff.find.close", message: "Close find" }),
  closeFindTitle: msg({ id: "diff.find.closeTitle", message: "Close (Esc)" }),
  resizeColumns: msg({
    id: "diff.columns.resize",
    message: "Resize old/new columns",
  }),
  resizeColumnsTitle: msg({
    id: "diff.columns.resizeTitle",
    message: "Drag to resize · double-click to reset",
  }),
} as const;
