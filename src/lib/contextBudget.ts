import type { FilePatch } from "../types";

export const CONTEXT_BYTE_BUDGET = 64 * 1024;
const encoder = new TextEncoder();
export const utf8Bytes = (text: string) => encoder.encode(text).byteLength;

export function boundedContext(text: string, limit = CONTEXT_BYTE_BUDGET): string {
  if (utf8Bytes(text) <= limit) return text;
  const marker = "\n[OMITTED: remaining content exceeds the clipboard byte budget]\n";
  const bytes = encoder.encode(text);
  let end = Math.max(0, limit - utf8Bytes(marker));
  while (end > 0 && (bytes[end] & 0xc0) === 0x80) end--;
  return new TextDecoder().decode(bytes.subarray(0, end)) + marker;
}

export function patchForClipboard(patch: FilePatch): string {
  return `${patch.oldRevision}:${patch.oldPath} → ${patch.newRevision}:${patch.newPath}\n${patch.text}`
    + (patch.truncated ? `\n[OMITTED: patch byte limit reached; this patch is incomplete]\n` : "");
}
