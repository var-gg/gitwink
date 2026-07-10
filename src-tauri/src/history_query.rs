//! Query primitives that respect published reachability epochs.
//!
//! An FTS hit is only a candidate. Every result is joined to the caller's
//! pinned membership epoch so stale rows left by a cancelled/force-push scan
//! can never masquerade as current repository history.

use anyhow::Result;
use rusqlite::{params, Connection};
use std::collections::HashSet;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SearchKey {
    pub(crate) repo_path: String,
    pub(crate) hash: String,
}

const MAX_FTS_TERMS: usize = 6;
const MAX_FTS_GRAMS: usize = 256;

/// Turn user text into a bounded AND of distinct three-scalar grams. The FTS
/// index uses `detail=none`, which stores only candidate rowids instead of
/// positional postings; the exact LIKE predicates remain the semantic
/// contract and reject out-of-order/cross-column false positives. Every term
/// must contain at least three Unicode scalar values, otherwise the entire
/// query deliberately stays on the exact LIKE path.
pub(crate) fn substring_expression(query: &str) -> Option<String> {
    if query.len() > 2_048 || query.chars().take(513).count() > 512 {
        return None;
    }
    let raw: Vec<&str> = query.split_whitespace().take(MAX_FTS_TERMS).collect();
    if raw.is_empty() || raw.iter().any(|term| term.chars().take(3).count() < 3) {
        return None;
    }
    let mut seen = HashSet::new();
    let mut grams = Vec::new();
    'terms: for term in raw {
        let mut chars = term.chars();
        let mut first = chars.next()?;
        let mut second = chars.next()?;
        for third in chars {
            let gram = [first, second, third].iter().collect::<String>();
            if seen.insert(gram.clone()) {
                grams.push(format!("\"{}\"", gram.replace('"', "\"\"")));
                if grams.len() == MAX_FTS_GRAMS {
                    break 'terms;
                }
            }
            first = second;
            second = third;
        }
    }
    if grams.is_empty() {
        None
    } else {
        Some(grams.join(" AND "))
    }
}

pub(crate) fn search_repo_substring(
    conn: &Connection,
    repo_path: &str,
    published_epoch: i64,
    query: &str,
    limit: usize,
) -> Result<Vec<SearchKey>> {
    let Some(expression) = substring_expression(query) else {
        return Ok(Vec::new());
    };
    let mut statement = conn.prepare(
        r#"
        SELECT c.repo_path, c.hash
        FROM commits_fts
        JOIN commits c ON c.rowid = commits_fts.rowid
        JOIN history_membership membership
          ON membership.repo_path = c.repo_path
         AND membership.hash = c.hash
         AND membership.scan_epoch = ?2
        WHERE commits_fts MATCH ?1
          AND c.repo_path = ?3
          AND EXISTS (
                SELECT 1 FROM repos
                WHERE (repos.path = c.repo_path OR repos.canonical_path = c.repo_path)
                  AND repos.status = 'active'
                  AND repos.user_state != 'removed'
          )
        ORDER BY bm25(commits_fts), c.sort_ts, c.hash
        LIMIT ?4
        "#,
    )?;
    let rows = statement
        .query_map(
            params![expression, published_epoch, repo_path, limit.max(1) as i64],
            |row| {
                Ok(SearchKey {
                    repo_path: row.get(0)?,
                    hash: row.get(1)?,
                })
            },
        )?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

pub(crate) fn count_repo_publication(
    conn: &Connection,
    repo_path: &str,
    published_epoch: i64,
) -> Result<i64> {
    conn.query_row(
        r#"
        SELECT COUNT(*) FROM history_membership
        WHERE repo_path = ?1 AND scan_epoch = ?2
        "#,
        params![repo_path, published_epoch],
        |row| row.get(0),
    )
    .map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history_schema::test_support;

    #[test]
    fn substring_expression_is_bounded_quoted_and_minimum_length() {
        assert_eq!(
            substring_expression("alpha beta"),
            Some("\"alp\" AND \"lph\" AND \"pha\" AND \"bet\" AND \"eta\"".to_string())
        );
        let expression = substring_expression("aaa bbb ccc ddd eee fff ignored").unwrap();
        assert!(!expression.contains("ignored"));
        assert_eq!(expression.matches(" AND ").count(), 5);
        assert_eq!(substring_expression("ab"), None);
        assert_eq!(substring_expression("alpha xy"), None);
        assert_eq!(substring_expression("   "), None);
        assert_eq!(substring_expression("aaaa"), Some("\"aaa\"".to_string()));
        let huge = "abcdefghijklmnopqrstuvwxyz".repeat(19);
        assert!(substring_expression(&huge).unwrap().split(" AND ").count() <= MAX_FTS_GRAMS);
        assert_eq!(substring_expression(&"x".repeat(513)), None);
    }

    #[test]
    fn fts_candidate_requires_published_membership() {
        let conn = test_support::v5_connection();
        test_support::insert_commit(&conn, "published1", "optimization published", "body");
        test_support::insert_commit(&conn, "staged2222", "optimization staged", "body");
        conn.execute(
            "INSERT INTO history_membership(repo_path, scan_epoch, hash) VALUES ('/repo', 3, 'published1')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO history_membership(repo_path, scan_epoch, hash) VALUES ('/repo', 4, 'staged2222')",
            [],
        )
        .unwrap();

        let epoch3 = search_repo_substring(&conn, "/repo", 3, "tim", 20).unwrap();
        assert_eq!(
            epoch3,
            vec![SearchKey {
                repo_path: "/repo".to_string(),
                hash: "published1".to_string(),
            }]
        );
        let epoch4 = search_repo_substring(&conn, "/repo", 4, "tim", 20).unwrap();
        assert_eq!(epoch4[0].hash, "staged2222");
        assert_eq!(count_repo_publication(&conn, "/repo", 3).unwrap(), 1);
    }
}
