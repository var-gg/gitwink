import { useLingui } from "@lingui/react/macro";

import { filterMessages } from "../../i18n/messages/filters";
import { translate } from "../../i18n/translate";
import type { WindowDays } from "../../types";
import { WINDOW_DAY_PRESETS } from "../TimeRangeChip";
import { SearchScopePageHeader } from "./SearchScopePageHeader";

export function TimeScopePage({
  value,
  onChange,
  onBack,
  onClose,
}: {
  value: WindowDays;
  onChange: (days: WindowDays) => void;
  onBack: () => void;
  onClose: () => void;
}) {
  const { i18n } = useLingui();
  const options: WindowDays[] = [...WINDOW_DAY_PRESETS, "all"];
  return (
    <div className="search-scope-page">
      <SearchScopePageHeader
        title={i18n._(filterMessages.searchScopeTime)}
        onBack={onBack}
      />
      <div className="chip-list">
        {options.map((option) => (
          <button
            key={String(option)}
            type="button"
            className={"chip-item" + (option === value ? " active" : "")}
            onClick={() => {
              onChange(option);
              onClose();
            }}
          >
            <span className="chip-item-name">
              {option === "all"
                ? i18n._(filterMessages.allTime)
                : option === 1
                  ? i18n._(filterMessages.last24Hours)
                  : translate(i18n, filterMessages.lastDays, { count: option })}
            </span>
          </button>
        ))}
      </div>
    </div>
  );
}
