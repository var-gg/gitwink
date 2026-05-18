import { useEffect, useRef, useState } from "react";
import type { Highlighter } from "shiki";

import { parseDiff, type DiffSide } from "../lib/diff";
import { getHighlighter, highlightLine, langForPath } from "../lib/highlight";

interface Props {
  text: string;
  /** File path so we can detect the language for Shiki. Optional — falls
   * back to plain monospace when missing or unknown. */
  filePath?: string;
}

function isDarkScheme(): boolean {
  return (
    typeof window !== "undefined" &&
    window.matchMedia?.("(prefers-color-scheme: dark)").matches
  );
}

export function SideBySideDiff({ text, filePath }: Props) {
  const { hunks } = parseDiff(text);
  const leftRef = useRef<HTMLDivElement | null>(null);
  const rightRef = useRef<HTMLDivElement | null>(null);

  const [highlighter, setHighlighter] = useState<Highlighter | null>(null);
  const [dark, setDark] = useState(isDarkScheme);

  const lang = filePath ? langForPath(filePath) : null;

  // Lazy-load Shiki on first mount that has a known language. Skipped
  // entirely for unknown extensions — saves a multi-MB download.
  useEffect(() => {
    if (!lang) return;
    let cancelled = false;
    void getHighlighter().then((hl) => {
      if (!cancelled) setHighlighter(hl);
    });
    return () => {
      cancelled = true;
    };
  }, [lang]);

  // React to OS theme changes while the window is open.
  useEffect(() => {
    if (typeof window === "undefined" || !window.matchMedia) return;
    const mq = window.matchMedia("(prefers-color-scheme: dark)");
    const onChange = () => setDark(mq.matches);
    mq.addEventListener?.("change", onChange);
    return () => mq.removeEventListener?.("change", onChange);
  }, []);

  // Sync horizontal scroll between the two columns — GitHub / GitLens
  // pattern.
  useEffect(() => {
    const l = leftRef.current;
    const r = rightRef.current;
    if (!l || !r) return;
    let syncing = false;
    function onL() {
      if (syncing) return;
      syncing = true;
      r!.scrollLeft = l!.scrollLeft;
      syncing = false;
    }
    function onR() {
      if (syncing) return;
      syncing = true;
      l!.scrollLeft = r!.scrollLeft;
      syncing = false;
    }
    l.addEventListener("scroll", onL);
    r.addEventListener("scroll", onR);
    return () => {
      l.removeEventListener("scroll", onL);
      r.removeEventListener("scroll", onR);
    };
  }, [hunks.length]);

  if (hunks.length === 0) {
    return <div className="sbs-empty">No textual diff.</div>;
  }

  return (
    <div className="sbs">
      <div className="sbs-cols">
        <div className="sbs-col" ref={leftRef}>
          {hunks.map((h, hi) => (
            <div key={hi}>
              <div className="sbs-hunk-header">{h.header}</div>
              {h.rows.map((r, ri) => (
                <Line
                  key={ri}
                  side={r.left}
                  kind="left"
                  highlighter={highlighter}
                  lang={lang}
                  dark={dark}
                />
              ))}
            </div>
          ))}
        </div>
        <div className="sbs-col" ref={rightRef}>
          {hunks.map((h, hi) => (
            <div key={hi}>
              <div className="sbs-hunk-header sbs-hunk-header-blank">&nbsp;</div>
              {h.rows.map((r, ri) => (
                <Line
                  key={ri}
                  side={r.right}
                  kind="right"
                  highlighter={highlighter}
                  lang={lang}
                  dark={dark}
                />
              ))}
            </div>
          ))}
        </div>
      </div>
    </div>
  );
}

interface LineProps {
  side: DiffSide;
  kind: "left" | "right";
  highlighter: Highlighter | null;
  lang: ReturnType<typeof langForPath>;
  dark: boolean;
}

function Line({ side, kind, highlighter, lang, dark }: LineProps) {
  const sign =
    side.type === "delete" ? "-" : side.type === "add" ? "+" : " ";

  const highlighted =
    highlighter && lang
      ? highlightLine(highlighter, side.text || " ", lang, dark)
      : null;

  return (
    <div
      className={`sbs-line sbs-${kind} ${side.type ?? "blank"}`}
      data-line-num={side.lineNum ?? ""}
      data-side={kind}
    >
      <span className="sbs-num">{side.lineNum ?? ""}</span>
      <span className="sbs-sign">{sign}</span>
      {highlighted ? (
        <span
          className="sbs-text sbs-text-shiki"
          // Shiki output is trusted — we built it locally from our diff text.
          dangerouslySetInnerHTML={{ __html: highlighted }}
        />
      ) : (
        <span className="sbs-text">{side.text || " "}</span>
      )}
    </div>
  );
}
