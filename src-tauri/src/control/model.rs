use std::fmt;

use serde::{Deserialize, Serialize};

use crate::git::CommitSummary;

pub const PROTOCOL_ID: &str = "gitwink.control.v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ControlRequest {
    pub protocol: String,
    pub request_id: String,
    pub source: ControlSource,
    pub invocation_cwd: String,
    pub intent: ControlIntent,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ControlSource {
    pub kind: String,
    pub name: String,
}

impl Default for ControlSource {
    fn default() -> Self {
        Self {
            kind: "cli".to_string(),
            name: "gitwinkctl".to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ControlIntent {
    ShowRange {
        repo: RepoLocator,
        base: String,
        head: String,
    },
    ShowWorktree {
        repo: RepoLocator,
    },
    ShowCommit {
        repo: RepoLocator,
        revision: String,
        presentation: Presentation,
    },
    ShowFileHistory {
        repo: RepoLocator,
        file_path: String,
        presentation: Presentation,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum RepoLocator {
    LocalPath { path: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Presentation {
    pub focus: bool,
    pub expand: bool,
}

impl Presentation {
    pub const fn commit_default() -> Self {
        Self {
            focus: true,
            expand: true,
        }
    }

    pub const fn file_history_default() -> Self {
        Self {
            focus: true,
            expand: false,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(
    tag = "status",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ControlDelivery {
    Ready {
        request_id: String,
        source: ControlSource,
        target: ResolvedTarget,
    },
    Error {
        request_id: String,
        source: ControlSource,
        error: ControlFailure,
    },
}

impl ControlDelivery {
    pub fn request_id(&self) -> &str {
        match self {
            Self::Ready { request_id, .. } | Self::Error { request_id, .. } => request_id,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ResolvedTarget {
    Review {
        request: crate::review_types::ReviewRequest,
    },
    Commit {
        repo_path: String,
        commit: CommitSummary,
        presentation: Presentation,
    },
    FileHistory {
        repo_path: String,
        file_path: String,
        presentation: Presentation,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ControlFailure {
    pub code: ControlErrorCode,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlErrorCode {
    InvalidArguments,
    RepoNotRegistered,
    RepoExcluded,
    RepoUnavailable,
    RevisionNotFound,
    InvalidFilePath,
    RepositoryChanged,
    InternalError,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControlError {
    pub code: ControlErrorCode,
    pub message: String,
}

impl ControlError {
    pub fn new(code: ControlErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    pub fn failure(&self) -> ControlFailure {
        ControlFailure {
            code: self.code,
            message: self.message.clone(),
        }
    }
}

impl fmt::Display for ControlError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for ControlError {}
