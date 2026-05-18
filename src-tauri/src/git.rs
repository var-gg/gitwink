// Read-only git access via `git2`. Implemented in D4.
//
// HARD RULE: every call here must be read-only. No commits, fetches, pushes,
// merges, or any operation that mutates a repo's state on disk.
