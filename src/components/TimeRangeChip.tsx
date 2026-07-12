import type { WindowDays } from "../types";
import { useLingui } from "@lingui/react/macro";
import { ChipDropdown } from "./ChipDropdown";
import { filterMessages } from "../i18n/messages/filters";
import { translate } from "../i18n/translate";

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

function labelFor(v: WindowDays, allTime: string): string {
  if (v === "all") return allTime;
  if (v === 1) return "24h";
  return `${v}d`;
}

export function TimeRangeChip({ open, onToggle, onClose, value, onChange }: Props) {
  const { i18n } = useLingui();
  const options: { value: WindowDays; label: string }[] = [
    ...WINDOW_DAY_PRESETS.map((days) => ({
      value: days as WindowDays,
      label:
        days === 1
          ? i18n._(filterMessages.last24Hours)
          : translate(i18n, filterMessages.lastDays, { count: days }),
    })),
    { value: "all", label: i18n._(filterMessages.allTime) },
  ];
  return (
    <ChipDropdown
      id="time"
      label={labelFor(value, i18n._(filterMessages.allTime))}
      open={open}
      onToggle={onToggle}
      onClose={onClose}
      align="right"
    >
      <div className="chip-list">
        {options.map((o) => (
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
