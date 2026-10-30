use crate::git_diff::ChangedFile;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ReviewRequest {
    SinceReviewed {
        repo_path: String,
    },
    Range {
        repo_path: String,
        base: String,
        head: String,
    },
    Worktree {
        repo_path: String,
    },
}
impl ReviewRequest {
    pub fn repo_path(&self) -> &str {
        match self {
            Self::SinceReviewed { repo_path }
            | Self::Range { repo_path, .. }
            | Self::Worktree { repo_path } => repo_path,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ReviewBaseline {
    pub identity: String,
    pub head: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewFile {
    pub id: String,
    pub group: String,
    #[serde(flatten)]
    pub file: ChangedFile,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewSnapshot {
    pub id: String,
    pub repo_path: String,
    pub repo_name: String,
    pub branch: String,
    pub identity: String,
    pub mode: String,
    pub base: Option<String>,
    pub head: Option<String>,
    pub live_head: Option<String>,
    pub baseline: Option<ReviewBaseline>,
    pub baseline_status: String,
    pub fingerprint: Option<String>,
    pub created_at: u64,
    pub files: Vec<ReviewFile>,
    pub can_mark_reviewed: bool,
}
