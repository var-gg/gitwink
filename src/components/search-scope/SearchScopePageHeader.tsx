import { useLingui } from "@lingui/react/macro";

import { filterMessages } from "../../i18n/messages/filters";

export function SearchScopePageHeader({
  title,
  onBack,
}: {
  title: string;
  onBack: () => void;
}) {
  const { i18n } = useLingui();
  return (
    <div className="search-scope-page-header">
      <button
        type="button"
        className="search-scope-back"
        onClick={onBack}
        title={i18n._(filterMessages.searchScopeBack)}
        aria-label={i18n._(filterMessages.searchScopeBack)}
      >
        ‹
      </button>
      <span>{title}</span>
    </div>
  );
}
