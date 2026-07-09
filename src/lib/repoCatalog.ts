import type { DiscoveredRepo, Repo, RepoStatusPayload } from "../types";

export type RepoCatalogAction =
  | { type: "replace"; repos: Repo[] }
  | { type: "discovered"; repo: DiscoveredRepo }
  | { type: "status"; payload: RepoStatusPayload };

/** Repository paths arriving from Rust are canonical identifiers. Keep them
 * exact: lower-casing would merge distinct repositories on case-sensitive
 * macOS volumes. Alternate user spellings converge through the next
 * listRepos() replacement after an explicit add. */
export function repoCatalogReducer(
  repos: Repo[],
  action: RepoCatalogAction,
): Repo[] {
  switch (action.type) {
    case "replace":
      return sortRepos(action.repos);
    case "discovered": {
      if (repos.some((repo) => repo.path === action.repo.path)) return repos;
      return sortRepos([
        ...repos,
        {
          id: 0,
          path: action.repo.path,
          name: action.repo.name,
          status: "active",
        },
      ]);
    }
    case "status": {
      const { canonicalPath, status } = action.payload;
      if (status === "removed") {
        const next = repos.filter((repo) => repo.path !== canonicalPath);
        return next.length === repos.length ? repos : next;
      }
      let changed = false;
      const next = repos.map((repo) => {
        if (repo.path !== canonicalPath || repo.status === status) return repo;
        changed = true;
        return { ...repo, status };
      });
      return changed ? next : repos;
    }
  }
}

/** Resolve a visible multi-repo selection to the cache row ids used by the
 * windowed timeline. `null` is reserved for the explicit all-repos state.
 * An unresolved path returns an empty list instead of silently widening the
 * query to every repo while the chip still claims a narrow scope. */
export function resolveRepoIds(
  selectedPaths: string[] | "all",
  repos: Repo[],
): number[] | null {
  if (selectedPaths === "all") return null;
  const byPath = new Map(repos.map((repo) => [repo.path, repo.id]));
  const ids: number[] = [];
  for (const path of selectedPaths) {
    const id = byPath.get(path);
    if (id == null || id <= 0) return [];
    ids.push(id);
  }
  return ids;
}

/** Canonical paths are exact identities on case-sensitive volumes. Windows
 * is the one platform where separator/case tolerance is appropriate for
 * matching a watcher/fetch event back to the current view. */
export function sameCanonicalPath(
  a: string,
  b: string,
  windows =
    typeof navigator !== "undefined" &&
    /win/i.test(navigator.platform),
): boolean {
  if (a === b) return true;
  const normalize = (path: string) =>
    path.replace(/\\/g, "/").replace(/\/+$/, "");
  const left = normalize(a);
  const right = normalize(b);
  return windows
    ? left.toLocaleLowerCase("en-US") === right.toLocaleLowerCase("en-US")
    : left === right;
}

function sortRepos(repos: Repo[]): Repo[] {
  const next = [...repos];
  next.sort(
    (a, b) => a.name.localeCompare(b.name) || a.path.localeCompare(b.path),
  );
  return next;
}
