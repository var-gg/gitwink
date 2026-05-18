import type { LaneGraphData } from "../lib/lanes";

interface Props {
  graph: LaneGraphData;
  rowHeight: number;
  /** Vertical offset to the center of the first row (for aligning with
   * the timeline-row baselines). */
  firstRowCenter: number;
}

const LANE_WIDTH = 12;
const CIRCLE_R = 3.5;

export function LaneGraph({ graph, rowHeight, firstRowCenter }: Props) {
  const width = Math.max(graph.totalLanes * LANE_WIDTH, LANE_WIDTH);
  const height = (graph.laneCommits.length - 1) * rowHeight + firstRowCenter * 2;

  function cx(lane: number): number {
    return lane * LANE_WIDTH + LANE_WIDTH / 2;
  }
  function cy(idx: number): number {
    return firstRowCenter + idx * rowHeight;
  }

  return (
    <svg
      className="lane-graph"
      width={width}
      height={Math.max(height, rowHeight)}
      viewBox={`0 0 ${width} ${Math.max(height, rowHeight)}`}
    >
      {/* Edges below circles. */}
      {graph.edges.map((e, i) => {
        const fromY = cy(e.fromIdx);
        const toY = e.toIdx >= 0 ? cy(e.toIdx) : height + 12;
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
            strokeOpacity={0.85}
          />
        );
      })}
      {graph.laneCommits.map((lc, i) => (
        <circle
          key={i}
          cx={cx(lc.lane)}
          cy={cy(i)}
          r={CIRCLE_R}
          fill={lc.color}
          stroke="rgba(0,0,0,0.22)"
          strokeWidth={0.5}
        />
      ))}
    </svg>
  );
}
