// Branch coloring for the DAG lane drawer. Eight slots, deterministic hash
// from branch name. `main` / `master` / `develop` / `dev` always get the
// neutral color so the dominant trunk doesn't randomly land on, say, red.

const PALETTE = [
  "#e85d75", // pink
  "#e8a85d", // orange
  "#d7c33b", // yellow
  "#65d685", // green
  "#5da5e8", // blue
  "#7c5de8", // indigo
  "#b75de8", // purple
  "#5dd6d6", // teal
];

const NEUTRAL = "#8b8b8b";

const NEUTRAL_BRANCHES = new Set(["main", "master", "develop", "dev", "trunk"]);

function hash(s: string): number {
  let h = 0;
  for (let i = 0; i < s.length; i++) {
    h = (h * 31 + s.charCodeAt(i)) | 0;
  }
  return Math.abs(h);
}

export function colorForBranch(name: string | null | undefined): string {
  if (!name) return NEUTRAL;
  if (NEUTRAL_BRANCHES.has(name)) return NEUTRAL;
  return PALETTE[hash(name) % PALETTE.length];
}
