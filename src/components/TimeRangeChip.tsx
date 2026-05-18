import type { WindowDays } from "../types";
import { ChipDropdown } from "./ChipDropdown";

interface Props {
  open: boolean;
  onToggle: () => void;
  onClose: () => void;
  value: WindowDays;
  onChange: (v: WindowDays) => void;
}

const OPTIONS: { value: WindowDays; label: string }[] = [
  { value: 1, label: "Last 24 hours" },
  { value: 3, label: "Last 3 days" },
  { value: 7, label: "Last 7 days" },
  { value: 30, label: "Last 30 days" },
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
