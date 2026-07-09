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

function sortRepos(repos: Repo[]): Repo[] {
  const next = [...repos];
  next.sort(
    (a, b) => a.name.localeCompare(b.name) || a.path.localeCompare(b.path),
  );
  return next;
}
