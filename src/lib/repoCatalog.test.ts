import { describe, expect, it } from "vitest";

import type { DiscoveredRepo, Repo } from "../types";
import {
  repoCatalogReducer,
  resolveRepoIds,
  sameCanonicalPath,
} from "./repoCatalog";

const alpha: Repo = {
  id: 1,
  path: "/repos/alpha",
  name: "alpha",
  status: "active",
};

function discovered(path: string, name: string): DiscoveredRepo {
  return { path, name, source: "manual", confidence: 100 };
}

describe("repoCatalogReducer", () => {
  it("keeps re-add discovery idempotent so list length is the repo count", () => {
    const once = repoCatalogReducer([alpha], {
      type: "discovered",
      repo: discovered(alpha.path, alpha.name),
    });
    const twice = repoCatalogReducer(once, {
      type: "discovered",
      repo: discovered(alpha.path, alpha.name),
    });

    expect(twice).toBe(once);
    expect(twice).toHaveLength(1);
  });

  it("makes repeated removed events idempotent", () => {
    const removed = repoCatalogReducer([alpha], {
      type: "status",
      payload: { canonicalPath: alpha.path, status: "removed" },
    });
    const echoed = repoCatalogReducer(removed, {
      type: "status",
      payload: { canonicalPath: alpha.path, status: "removed" },
    });

    expect(removed).toEqual([]);
    expect(echoed).toBe(removed);
  });

  it("preserves case-distinct canonical paths", () => {
    const upper = discovered("/repos/Foo", "Foo");
    const lower = discovered("/repos/foo", "foo");
    const first = repoCatalogReducer([], { type: "discovered", repo: upper });
    const both = repoCatalogReducer(first, { type: "discovered", repo: lower });
    const oneRemoved = repoCatalogReducer(both, {
      type: "status",
      payload: { canonicalPath: upper.path, status: "removed" },
    });

    expect(both).toHaveLength(2);
    expect(oneRemoved.map((repo) => repo.path)).toEqual([lower.path]);
  });

  it("never widens an unresolved repo selection to all repos", () => {
    expect(resolveRepoIds([alpha.path], [{ ...alpha, id: 0 }])).toEqual([]);
    expect(resolveRepoIds(["/repos/missing"], [alpha])).toEqual([]);
    expect(resolveRepoIds([alpha.path], [alpha])).toEqual([alpha.id]);
    expect(resolveRepoIds("all", [alpha])).toBeNull();
  });

  it("matches path case only on Windows", () => {
    expect(sameCanonicalPath("/repos/Foo", "/repos/foo", false)).toBe(false);
    expect(sameCanonicalPath("C:\\Repos\\Foo", "c:/repos/foo/", true)).toBe(
      true,
    );
  });
});
