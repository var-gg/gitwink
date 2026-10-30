import { useState } from "react";
import { useLingui } from "@lingui/react";
import { reviewMessages as m } from "../../i18n/messages/review";
import { openReview } from "../../lib/review/ipc";

export function ReviewLauncher({ repoPath }: { repoPath: string }) {
  const { i18n } = useLingui();
  const [error, setError] = useState<string | null>(null);
  return <span className="review-launcher">
    <button className="chip" type="button" onClick={() => {
      setError(null);
      void openReview({ kind: "sinceReviewed", repoPath }).catch(e => setError(String(e)));
    }}>{i18n._(m.open)}</button>
    {error && <span role="alert">{error}</span>}
  </span>;
}
