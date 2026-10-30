import type { ReviewFile } from "./ipc";

/** Untracked content needs a deliberate user selection, even when sorted first. */
export function initialReviewSelection(files: ReviewFile[]) {
  const tracked = files.filter(file => file.group !== "untracked");
  return { active: tracked[0]?.id ?? null, selected: new Set(tracked.map(file => file.id)) };
}
