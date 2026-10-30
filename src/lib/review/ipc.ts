import { invoke } from "@tauri-apps/api/core";
import type { ChangedFile, FilePatch } from "../../types";

export type ReviewRequest = { kind: "sinceReviewed" | "worktree"; repoPath: string }
  | { kind: "range"; repoPath: string; base: string; head: string };
export interface ReviewFile extends ChangedFile { id: string; group: string }
export interface ReviewSnapshot {
  id: string; repoPath: string; repoName: string; branch: string; identity: string;
  mode: string; base: string | null; head: string | null; liveHead: string | null;
  baseline: { identity: string; head: string } | null;
  baselineStatus: string; fingerprint: string | null; createdAt: number;
  files: ReviewFile[]; canMarkReviewed: boolean;
}
export const openReview = (request: ReviewRequest) => invoke<void>("open_review", { request });
export const takePendingReview = () => invoke<ReviewRequest | null>("take_pending_review");
export const reviewSnapshot = (request: ReviewRequest) => invoke<ReviewSnapshot>("review_snapshot", { request });
export const reviewPatch = (snapshotId: string, fileId: string, context = 3) =>
  invoke<FilePatch>("review_patch", { snapshotId, fileId, context });
export const validateReview = (snapshotId: string) => invoke<void>("review_validate", { snapshotId });
export const reviewCopyPatches = (snapshotId: string, fileIds: string[]) =>
  invoke<Record<string, FilePatch>>("review_copy_patches", { snapshotId, fileIds });
export const markReviewed = (snapshotId: string) => invoke<void>("review_mark_reviewed", { snapshotId });
