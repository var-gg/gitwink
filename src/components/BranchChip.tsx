import { useCallback, useEffect, useMemo, useState } from "react";

import type { BranchInfo } from "../types";
import { ChipDropdown } from "./ChipDropdown";
import { VirtualChipList, type VirtualChipRow } from "./VirtualChipList";
import { chipRowH, useUiScale } from "../lib/settings";

// Virtualised-row BASE heights (px) at scale 1.0 — chipRowH scales them
// against the current --ui-scale so JS row heights match the CSS content.
const ITEM_H_BASE = 26; // .chip-item — one-line branch entry
const HEADER_H_BASE = 25; // .chip-section-header — "Local" / "Remote tracking"
const EMPTY_H_BASE = 34; // .chip-empty — "No branches match."

interface Props {
  open: boolean;
  onToggle: () => void;
  onClose: () => void;
  branches: BranchInfo[];
  /** Array of refNames (e.g. "refs/heads/main", "refs/remotes/origin/main")
   * or the "all" sentinel. We key by refName so a local "main" and a
   * remote "origin/main" never collide. */
  selected: string[] | "all";
  onChange: (sel: string[] | "all") => void;
}

export function BranchChip({
  open,
  onToggle,
  onClose,
  branches,
  selected,
  onChange,
}: Props) {
  const [query, setQuery] = useState("");
  const scale = useUiScale();

  useEffect(() => {
    if (!open) setQuery("");
  }, [open]);

  // Snapshot of `selected` taken when the dropdown opens. The list order
  // is frozen against this — toggling a ✓ while the dropdown is open
  // never makes a row jump under the cursor. Reopening re-snapshots, so
  // the just-checked branches float to the top then (VS Code's pattern).
  const [snapshot, setSnapshot] = useState<string[] | "all">(selected);
  useEffect(() => {
    if (open) setSnapshot(selected);
    // `selected` is intentionally omitted: the snapshot must NOT update
    // while the dropdown stays open.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open]);

  const { localBranches, remoteBranches } = useMemo(() => {
    const q = query.trim().toLowerCase();
    const match = (b: BranchInfo) => !q || b.name.toLowerCase().includes(q);
    const snapSet = snapshot === "all" ? null : new Set(snapshot);
    // Branches selected at open-time sort to the top of their section.
    // The sort key is the snapshot, so the order holds steady while open.
    const rank = (b: BranchInfo) => (snapSet?.has(b.refName) ? 0 : 1);
    const bySnapshot = (a: BranchInfo, b: BranchInfo) => rank(a) - rank(b);
    return {
      localBranches: branches
        .filter((b) => b.kind === "local" && match(b))
        .sort(bySnapshot),
      remoteBranches: branches
        .filter((b) => b.kind === "remote" && match(b))
        .sort(bySnapshot),
    };
  }, [branches, query, snapshot]);

  // Label adapts to the count. For a single selection we show the branch
  // name itself — minimum cognitive load — and fall back to a count when
  // multiple are picked.
  const label = useMemo(() => {
    if (selected === "all") return `All branches (${branches.length})`;
    if (selected.length === 0) return "No branches";
    if (selected.length === 1) {
      const only = branches.find((b) => b.refName === selected[0]);
      return only?.name ?? "1 branch";
    }
    return `${selected.length} branches`;
  }, [selected, branches]);

  const toggle = useCallback(
    (refName: string) => {
      if (selected === "all") {
        // GitLens / IDE sidebar pattern: clicking a branch from the "all"
        // state means "focus on THIS one". The previous "everything except
        // this one" behaviour was a multi-select-with-checkboxes mental
        // model that doesn't match what users expect from a branch filter.
        onChange([refName]);
        return;
      }
      const set = new Set(selected);
      if (set.has(refName)) set.delete(refName);
      else set.add(refName);
      const next = Array.from(set);
      onChange(next.length === branches.length ? "all" : next);
    },
    [selected, branches, onChange],
  );

  // Flatten the two sections into one virtual-row list. The Local / Remote
  // headers and the empty-state line are known-height special rows
  // interleaved with the branch rows.
  const rows = useMemo<VirtualChipRow[]>(() => {
    const ITEM_H = chipRowH(scale, ITEM_H_BASE);
    const HEADER_H = chipRowH(scale, HEADER_H_BASE);
    const EMPTY_H = chipRowH(scale, EMPTY_H_BASE);

    const branchRow = (b: BranchInfo): VirtualChipRow => {
      // In the "all" meta-state the All branches row at the top carries the
      // highlight — individual items shouldn't ALSO look "checked", or a
      // user clicking a row to "uncheck it" is met with the GitLens focus
      // behaviour and it feels like a different row got deselected. So: no
      // per-item ✓ until the user makes an explicit selection.
      const isSelected =
        selected !== "all" && (selected as string[]).includes(b.refName);
      return {
        key: "branch:" + b.refName,
        height: ITEM_H,
        render: () => (
          <button
            type="button"
            className={"chip-item" + (isSelected ? " checked" : "")}
            onClick={() => toggle(b.refName)}
          >
            <span className="chip-check">{isSelected ? "✓" : ""}</span>
            <span className="chip-item-name">
              {b.name}
              {b.isHead && <span className="chip-item-head"> · HEAD</span>}
            </span>
            <span className="chip-item-count">{b.commitCount}</span>
          </button>
        ),
      };
    };

    const out: VirtualChipRow[] = [];
    out.push({
      key: "__all",
      height: ITEM_H,
      render: () => (
        <button
          type="button"
          className={"chip-item" + (selected === "all" ? " active" : "")}
          onClick={() => {
            onChange("all");
            onClose();
          }}
        >
          <span className="chip-item-name">All branches</span>
        </button>
      ),
    });
    if (localBranches.length > 0) {
      out.push({
        key: "__local",
        height: HEADER_H,
        render: () => <div className="chip-section-header">Local</div>,
      });
      for (const b of localBranches) out.push(branchRow(b));
    }
    if (remoteBranches.length > 0) {
      out.push({
        key: "__remote",
        height: HEADER_H,
        render: () => (
          <div
            className="chip-section-header"
            title="Remote-tracking refs are local — gitwink never calls git fetch. Updated by your IDE / CLI."
          >
            Remote tracking
          </div>
        ),
      });
      for (const b of remoteBranches) out.push(branchRow(b));
    }
    if (localBranches.length === 0 && remoteBranches.length === 0) {
      out.push({
        key: "__empty",
        height: EMPTY_H,
        render: () => <div className="chip-empty">No branches match.</div>,
      });
    }
    return out;
  }, [localBranches, remoteBranches, selected, onChange, onClose, toggle, scale]);

  return (
    <ChipDropdown
      id="branch"
      label={label}
      open={open}
      onToggle={onToggle}
      onClose={onClose}
    >
      <div className="chip-search">
        <input
          autoFocus
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          placeholder="Search branches…"
        />
      </div>
      <VirtualChipList rows={rows} resetKey={query} />
    </ChipDropdown>
  );
}
