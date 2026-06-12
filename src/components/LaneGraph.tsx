import type { LaneCommit, LaneEdge } from "../lib/lanes";
import type { CommitSummary } from "../types";

interface Props {
  /** Full lane-commit list (newest-first) — sliced to the visible band. */
  laneCommits: LaneCommit[];
  /** Edges already filtered to those intersecting the visible band. */
  edges: LaneEdge[];
  /** Lane count — sets the SVG width. */
  totalLanes: number;
  /** First visible commit index (inclusive, overscan included). */
  first: number;
  /** One past the last visible commit index. */
  last: number;
  /** Content-Y centre of commit row `idx`. */
  cy: (idx: number) => number;
  /** Content-Y an off-window parent edge runs down to (the list bottom). */
  bottomY: number;
  /** Content-Y of the SVG's top edge — band coords are content − bandTop. */
  bandTop: number;
  /** SVG height in px. */
  bandHeight: number;
}

const LANE_WIDTH = 12;
const CIRCLE_R = 3.5;

/** Same glyph + label set the all-mode marker uses, so hovering a DAG
 * node in single-repo mode conveys the same info — type, summary, and
 * a short hash for orientation. Single-repo mode renders the marker
 * column as a spacer (the SVG IS the marker), so without this title
 * the hover affordance is silently lost. */
function commitTooltip(commit: CommitSummary): string {
  const glyph = commit.isTagged ? "★" : commit.isMerge ? "◆" : "●";
  const label = commit.isTagged
    ? "Tagged commit"
    : commit.isMerge
      ? "Merge commit"
      : "Commit";
  const summary = commit.summary || "(no summary)";
  return `${glyph} ${label} · ${commit.shortHash}\n${summary}`;
}

/** The single-repo DAG, windowed: only the nodes + edges intersecting the
 * visible band are emitted as SVG, and coordinates are local to the band
 * (content-Y − bandTop) so they stay small + precise no matter how deep
 * the repo's history runs. The SVG viewport clips the off-band tails of
 * edges that merely cross the band. */
export function LaneGraph({
  laneCommits,
  edges,
  totalLanes,
  first,
  last,
  cy,
  bottomY,
  bandTop,
  bandHeight,
}: Props) {
  const width = Math.max(totalLanes * LANE_WIDTH, LANE_WIDTH);
  const height = Math.max(bandHeight, 0);
  const cx = (lane: number) => lane * LANE_WIDTH + LANE_WIDTH / 2;
  const localY = (contentY: number) => contentY - bandTop;

  return (
    <svg
      className="lane-graph"
      width={width}
      height={height}
      viewBox={`0 0 ${width} ${height}`}
      style={{ top: bandTop }}
    >
      {edges.map((e, i) => {
        const fromY = localY(cy(e.fromIdx));
        const toY = localY(e.toIdx >= 0 ? cy(e.toIdx) : bottomY);
        const fromX = cx(e.fromLane);
        const toX = cx(e.toLane);
        const d =
          fromX === toX
            ? `M${fromX},${fromY} L${toX},${toY}`
            : `M${fromX},${fromY} C${fromX},${(fromY + toY) / 2} ${toX},${(fromY + toY) / 2} ${toX},${toY}`;
        return (
          <path
            key={i}
            d={d}
            stroke={e.color}
            strokeWidth={1.5}
            fill="none"
            // Bridged = hidden commits elided between child and parent
            // (author-filtered view) — dashed and dimmer, so a direct
            // parent link stays visually distinct.
            strokeOpacity={e.bridged ? 0.55 : 0.85}
            strokeDasharray={e.bridged ? "4 3" : undefined}
          />
        );
      })}
      {laneCommits.slice(first, last).map((lc, i) => {
        const idx = first + i;
        return (
          <circle
            key={idx}
            cx={cx(lc.lane)}
            cy={localY(cy(idx))}
            r={CIRCLE_R}
            fill={lc.color}
            stroke="rgba(0,0,0,0.22)"
            strokeWidth={0.5}
          >
            <title>{commitTooltip(lc.commit)}</title>
          </circle>
        );
      })}
    </svg>
  );
}
