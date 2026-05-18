import { useEffect, useRef } from "react";

import { parseDiff, type DiffSide } from "../lib/diff";

interface Props {
  text: string;
}

export function SideBySideDiff({ text }: Props) {
  const { hunks } = parseDiff(text);
  const leftRef = useRef<HTMLDivElement | null>(null);
  const rightRef = useRef<HTMLDivElement | null>(null);

  // Sync horizontal scroll between the two columns the way GitHub /
  // GitLens does — dragging one scrolls the other in lock-step.
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
                <Line key={ri} side={r.left} kind="left" />
              ))}
            </div>
          ))}
        </div>
        <div className="sbs-col" ref={rightRef}>
          {hunks.map((h, hi) => (
            <div key={hi}>
              <div className="sbs-hunk-header sbs-hunk-header-blank">&nbsp;</div>
              {h.rows.map((r, ri) => (
                <Line key={ri} side={r.right} kind="right" />
              ))}
            </div>
          ))}
        </div>
      </div>
    </div>
  );
}

function Line({ side, kind }: { side: DiffSide; kind: "left" | "right" }) {
  const sign =
    side.type === "delete" ? "-" : side.type === "add" ? "+" : " ";
  return (
    <div
      className={`sbs-line sbs-${kind} ${side.type ?? "blank"}`}
      data-line-num={side.lineNum ?? ""}
      data-side={kind}
    >
      <span className="sbs-num">{side.lineNum ?? ""}</span>
      <span className="sbs-sign">{sign}</span>
      <span className="sbs-text">{side.text || " "}</span>
    </div>
  );
}
