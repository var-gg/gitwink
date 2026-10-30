import { useEffect, useMemo, useState } from "react";
import { parseDiff } from "./diff";
import { flattenDiff } from "./diffView";

const EMPTY = flattenDiff([]);
const WORKER_THRESHOLD = 128 * 1024;

/** Cancel obsolete parsing instead of letting a large previous file block the next. */
export function useParsedDiff(text: string) {
  const inline = useMemo(() => text.length < WORKER_THRESHOLD || typeof Worker === "undefined"
    ? flattenDiff(parseDiff(text).hunks) : null, [text]);
  const [parsed, setParsed] = useState(EMPTY);
  const [completedText, setCompletedText] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    setError(null);
    if (inline) { setParsed(inline); return; }
    const worker = new Worker(new URL("./diffParse.worker.ts", import.meta.url), { type: "module" });
    worker.onmessage = (event) => { setParsed(event.data); setCompletedText(text); worker.terminate(); };
    worker.onerror = () => { setError("Diff parsing failed"); worker.terminate(); };
    worker.postMessage(text);
    return () => worker.terminate();
  }, [text, inline]);
  return { ...(inline ?? parsed), parseError: error, parsing: !inline && completedText !== text };
}
