// Relative-time formatting shared across chips/lists.
//
// `compactAge` renders a Unix-seconds timestamp as a short age string
// ("3h", "5d", "8mo", "2y") — used for the BranchChip's "last activity"
// column where horizontal space is tight. Branches can be very old, so it
// extends past days into weeks / months / years (unlike the timeline's
// day-capped helper).

export function compactAge(unixSeconds: number): string {
  const diff = Math.max(0, Math.floor(Date.now() / 1000) - unixSeconds);
  if (diff < 60) return "now";
  if (diff < 3600) return `${Math.floor(diff / 60)}m`;
  if (diff < 86_400) return `${Math.floor(diff / 3600)}h`;
  if (diff < 7 * 86_400) return `${Math.floor(diff / 86_400)}d`;
  if (diff < 30 * 86_400) return `${Math.floor(diff / (7 * 86_400))}w`;
  if (diff < 365 * 86_400) return `${Math.floor(diff / (30 * 86_400))}mo`;
  return `${Math.floor(diff / (365 * 86_400))}y`;
}

/** Full local date-time, for the tooltip behind a compact age. */
export function fullDateTime(unixSeconds: number, locale?: string): string {
  return new Date(unixSeconds * 1000).toLocaleString(locale, {
    year: "numeric",
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
  });
}
