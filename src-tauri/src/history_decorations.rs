//! Refresh mutable tag/remote-tip decorations after an additive history scan.
//!
//! Commit text is immutable, but ref decorations are not. A delta walk hides
//! the published graph, so a new tag/remote pointing to an old commit would
//! otherwise never touch that row.

use std::collections::{HashMap, HashSet};

use anyhow::Result;
use rusqlite::{params, Connection};

use crate::history_git::{HistoryRootKind, RootSnapshot};

pub(crate) fn refresh_tip_decorations(
    conn: &Connection,
    repo_path: &str,
    published: &RootSnapshot,
    current: &RootSnapshot,
) -> Result<()> {
    let old_tags: HashSet<&str> = published
        .roots
        .iter()
        .filter(|root| root.kind == HistoryRootKind::Tag)
        .map(|root| root.oid.as_str())
        .collect();
    let old_remote_tips: HashSet<&str> = published
        .roots
        .iter()
        .filter(|root| root.kind == HistoryRootKind::Remote)
        .map(|root| root.oid.as_str())
        .collect();
    for oid in old_tags {
        conn.execute(
            "UPDATE commits SET is_tagged = 0 WHERE repo_path = ?1 AND hash = ?2",
            params![repo_path, oid],
        )?;
    }
    for oid in old_remote_tips {
        conn.execute(
            r#"
            UPDATE commits
            SET remote_tip_label = NULL, remote_tip_extra_count = 0
            WHERE repo_path = ?1 AND hash = ?2
            "#,
            params![repo_path, oid],
        )?;
    }

    let new_tags: HashSet<&str> = current
        .roots
        .iter()
        .filter(|root| root.kind == HistoryRootKind::Tag)
        .map(|root| root.oid.as_str())
        .collect();
    for oid in new_tags {
        conn.execute(
            "UPDATE commits SET is_tagged = 1 WHERE repo_path = ?1 AND hash = ?2",
            params![repo_path, oid],
        )?;
    }
    let mut remotes: HashMap<&str, Vec<String>> = HashMap::new();
    for root in current
        .roots
        .iter()
        .filter(|root| root.kind == HistoryRootKind::Remote)
    {
        remotes.entry(root.oid.as_str()).or_default().push(
            root.ref_name
                .trim_start_matches("refs/remotes/")
                .to_string(),
        );
    }
    for (oid, mut labels) in remotes {
        labels.sort();
        labels.dedup();
        conn.execute(
            r#"
            UPDATE commits
            SET remote_tip_label = ?3, remote_tip_extra_count = ?4
            WHERE repo_path = ?1 AND hash = ?2
            "#,
            params![
                repo_path,
                oid,
                labels.first().map(String::as_str),
                labels.len().saturating_sub(1) as i64
            ],
        )?;
    }
    Ok(())
}

/// First complete publication has no prior `history_roots` snapshot. Clear
/// only rows that actually carry mutable decorations, then install the new
/// tips in the same publication transaction. Staged metadata never mutates a
/// currently-visible row before this edge.
pub(crate) fn replace_tip_decorations(
    conn: &Connection,
    repo_path: &str,
    current: &RootSnapshot,
) -> Result<()> {
    conn.execute(
        r#"
        UPDATE commits
        SET is_tagged = 0, remote_tip_label = NULL,
            remote_tip_extra_count = 0
        WHERE repo_path = ?1
          AND (is_tagged != 0 OR remote_tip_label IS NOT NULL
               OR remote_tip_extra_count != 0)
        "#,
        params![repo_path],
    )?;
    refresh_tip_decorations(
        conn,
        repo_path,
        &RootSnapshot::from_roots(Vec::new()),
        current,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history_git::{HistoryRoot, RootSnapshot};
    use crate::history_schema::test_support;

    fn root(name: &str, oid: &str, kind: HistoryRootKind) -> HistoryRoot {
        HistoryRoot {
            ref_name: name.to_string(),
            oid: oid.to_string(),
            kind,
        }
    }

    #[test]
    fn delta_refresh_moves_tag_and_remote_tip_without_scanning_history() {
        let conn = test_support::v5_connection();
        test_support::insert_commit(&conn, "old1111", "old", "old");
        test_support::insert_commit(&conn, "new2222", "new", "new");
        conn.execute(
            "UPDATE commits SET is_tagged = 1, remote_tip_label = 'origin/main' WHERE hash = 'old1111'",
            [],
        )
        .unwrap();
        let published = RootSnapshot::from_roots(vec![
            root("refs/tags/v1", "old1111", HistoryRootKind::Tag),
            root(
                "refs/remotes/origin/main",
                "old1111",
                HistoryRootKind::Remote,
            ),
        ]);
        let current = RootSnapshot::from_roots(vec![
            root("refs/tags/v2", "new2222", HistoryRootKind::Tag),
            root(
                "refs/remotes/origin/main",
                "new2222",
                HistoryRootKind::Remote,
            ),
            root(
                "refs/remotes/upstream/main",
                "new2222",
                HistoryRootKind::Remote,
            ),
        ]);

        refresh_tip_decorations(&conn, "/repo", &published, &current).unwrap();
        let old: (i64, Option<String>, i64) = conn
            .query_row(
                "SELECT is_tagged, remote_tip_label, remote_tip_extra_count FROM commits WHERE hash = 'old1111'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        let new: (i64, Option<String>, i64) = conn
            .query_row(
                "SELECT is_tagged, remote_tip_label, remote_tip_extra_count FROM commits WHERE hash = 'new2222'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(old, (0, None, 0));
        assert_eq!(new, (1, Some("origin/main".to_string()), 1));
    }
}
