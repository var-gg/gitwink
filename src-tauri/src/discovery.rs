// Repo discovery — implemented in D3.
//
// Responsibilities:
//   - Walk default user directories with `ignore` + `walkdir`.
//   - Honor .gitignore-style rules, hard-exclude vendor dirs, cap depth.
//   - Stop descending into a directory once a `.git` is found.
//   - Persist results via `cache`.
