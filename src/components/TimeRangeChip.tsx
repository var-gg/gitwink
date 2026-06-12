import type { WindowDays } from "../types";
import { ChipDropdown } from "./ChipDropdown";

interface Props {
  open: boolean;
  onToggle: () => void;
  onClose: () => void;
  value: WindowDays;
  onChange: (v: WindowDays) => void;
}

/** The numeric presets, smallest first — the single source both for the
 * dropdown options below and for the warp's window-widening pick
 * (App.windowCovering). Keeping them in one place means adding a preset
 * can't silently desync the two. */
export const WINDOW_DAY_PRESETS = [1, 3, 7, 30] as const;

const LABELS: Record<number, string> = {
  1: "Last 24 hours",
  3: "Last 3 days",
  7: "Last 7 days",
  30: "Last 30 days",
};

const OPTIONS: { value: WindowDays; label: string }[] = [
  ...WINDOW_DAY_PRESETS.map((d) => ({
    value: d as WindowDays,
    label: LABELS[d] ?? `Last ${d} days`,
  })),
  { value: "all", label: "All time" },
];

function labelFor(v: WindowDays): string {
  if (v === "all") return "All time";
  if (v === 1) return "24h";
  return `${v}d`;
}

export function TimeRangeChip({ open, onToggle, onClose, value, onChange }: Props) {
  return (
    <ChipDropdown
      id="time"
      label={labelFor(value)}
      open={open}
      onToggle={onToggle}
      onClose={onClose}
      align="right"
    >
      <div className="chip-list">
        {OPTIONS.map((o) => (
          <button
            key={String(o.value)}
            type="button"
            className={"chip-item" + (o.value === value ? " active" : "")}
            onClick={() => {
              onChange(o.value);
              onClose();
            }}
          >
            <span className="chip-item-name">{o.label}</span>
          </button>
        ))}
      </div>
    </ChipDropdown>
  );
}
