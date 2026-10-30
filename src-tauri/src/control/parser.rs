use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use super::model::{
    ControlError, ControlErrorCode, ControlIntent, ControlRequest, ControlSource, Presentation,
    RepoLocator, PROTOCOL_ID,
};

const MAX_ARGUMENTS: usize = 64;
const MAX_ARGUMENT_BYTES: usize = 32 * 1024;
const MAX_REVISION_BYTES: usize = 512;
const MAX_SOURCE_BYTES: usize = 48;
const MAX_REQUEST_ID_BYTES: usize = 96;

static REQUEST_COUNTER: AtomicU64 = AtomicU64::new(0);

pub fn new_request_id() -> String {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let sequence = REQUEST_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("local-{millis:x}-{:x}-{sequence:x}", std::process::id())
}

/// Parse arguments received by the GUI executable. A normal tray launch is
/// not a control request and returns `Ok(None)`; once `show` is recognized,
/// malformed input is an explicit error that the panel can surface.
pub fn parse_app_argv(argv: &[String], cwd: &str) -> Result<Option<ControlRequest>, ControlError> {
    let tokens = match argv {
        [first, ..] if first == "show" => argv,
        [_, second, ..] if second == "show" => &argv[1..],
        _ => return Ok(None),
    };
    if tokens.len() > MAX_ARGUMENTS {
        return Err(invalid("Too many command-line arguments"));
    }
    parse_show_tokens(tokens, cwd).map(Some)
}

pub fn parse_show_tokens(tokens: &[String], cwd: &str) -> Result<ControlRequest, ControlError> {
    validate_argument_bounds(tokens)?;
    if tokens.first().map(String::as_str) != Some("show") {
        return Err(invalid("Expected `show commit` or `show file-history`"));
    }
    let target = tokens
        .get(1)
        .map(String::as_str)
        .ok_or_else(|| invalid("Missing show target (`commit` or `file-history`)"))?;

    let mut repo: Option<String> = None;
    let mut base = None;
    let mut head = None;
    let mut revision: Option<String> = None;
    let mut file_path: Option<String> = None;
    let mut source: Option<String> = None;
    let mut request_id: Option<String> = None;
    let mut expand = target == "commit";
    let mut expand_explicit = false;

    let mut index = 2usize;
    while index < tokens.len() {
        match tokens[index].as_str() {
            "--base" => set_once(
                &mut base,
                value_after(tokens, &mut index, "--base")?,
                "--base",
            )?,
            "--head" => set_once(
                &mut head,
                value_after(tokens, &mut index, "--head")?,
                "--head",
            )?,
            "--repo" => set_once(
                &mut repo,
                value_after(tokens, &mut index, "--repo")?,
                "--repo",
            )?,
            "--rev" => set_once(
                &mut revision,
                value_after(tokens, &mut index, "--rev")?,
                "--rev",
            )?,
            "--path" => set_once(
                &mut file_path,
                value_after(tokens, &mut index, "--path")?,
                "--path",
            )?,
            "--source" => set_once(
                &mut source,
                value_after(tokens, &mut index, "--source")?,
                "--source",
            )?,
            "--request-id" => set_once(
                &mut request_id,
                value_after(tokens, &mut index, "--request-id")?,
                "--request-id",
            )?,
            "--expand" => {
                expand = true;
                expand_explicit = true;
            }
            "--no-expand" => {
                expand = false;
                expand_explicit = true;
            }
            unknown => return Err(invalid(format!("Unknown argument `{unknown}`"))),
        }
        index += 1;
    }

    let repo = non_empty(repo.unwrap_or_else(|| ".".to_string()), "--repo")?;
    let source = sanitize_source(source.unwrap_or_else(|| "gitwinkctl".to_string()))?;
    let request_id = sanitize_request_id(request_id.unwrap_or_else(new_request_id))?;
    let invocation_cwd = non_empty(cwd.to_string(), "invocation cwd")?;
    let repo = RepoLocator::LocalPath { path: repo };

    if target != "range" && (base.is_some() || head.is_some()) {
        return Err(invalid(
            "`--base` and `--head` are only valid for `show range`",
        ));
    }
    let intent = match target {
        "range" | "worktree" => {
            if revision.is_some() || file_path.is_some() || expand_explicit {
                return Err(invalid("Range/worktree do not accept --rev, --path or --expand"));
            }
            if target == "worktree" { ControlIntent::ShowWorktree { repo } }
            else {
                let base = non_empty(base.ok_or_else(|| invalid("show range requires --base"))?, "--base")?;
                let head = non_empty(head.ok_or_else(|| invalid("show range requires --head"))?, "--head")?;
                if base.len() > MAX_REVISION_BYTES || head.len() > MAX_REVISION_BYTES { return Err(invalid("Revision is too long")); }
                ControlIntent::ShowRange { repo, base, head }
            }
        }
        "commit" => {
            if file_path.is_some() {
                return Err(invalid("`--path` is only valid for `show file-history`"));
            }
            let revision = non_empty(
                revision.ok_or_else(|| invalid("`show commit` requires `--rev <revision>`"))?,
                "--rev",
            )?;
            if revision.len() > MAX_REVISION_BYTES {
                return Err(invalid("Revision is too long"));
            }
            ControlIntent::ShowCommit {
                repo,
                revision,
                presentation: Presentation {
                    focus: true,
                    expand,
                },
            }
        }
        "file-history" => {
            if revision.is_some() {
                return Err(invalid("`--rev` is only valid for `show commit`"));
            }
            if expand_explicit {
                return Err(invalid(
                    "`--expand` and `--no-expand` are only valid for `show commit`",
                ));
            }
            let file_path = non_empty(
                file_path.ok_or_else(|| invalid("`show file-history` requires `--path <file>`"))?,
                "--path",
            )?;
            ControlIntent::ShowFileHistory {
                repo,
                file_path,
                presentation: Presentation::file_history_default(),
            }
        }
        unknown => {
            return Err(invalid(format!(
                "Unknown show target `{unknown}`; expected `commit`, `file-history`, `range` or `worktree`"
            )))
        }
    };

    Ok(ControlRequest {
        protocol: PROTOCOL_ID.to_string(),
        request_id,
        source: ControlSource {
            kind: "cli".to_string(),
            name: source,
        },
        invocation_cwd,
        intent,
    })
}

fn validate_argument_bounds(tokens: &[String]) -> Result<(), ControlError> {
    if tokens.len() > MAX_ARGUMENTS {
        return Err(invalid("Too many command-line arguments"));
    }
    if tokens
        .iter()
        .any(|argument| argument.len() > MAX_ARGUMENT_BYTES || argument.as_bytes().contains(&0))
    {
        return Err(invalid("An argument is empty, too long, or contains NUL"));
    }
    Ok(())
}

fn value_after(tokens: &[String], index: &mut usize, flag: &str) -> Result<String, ControlError> {
    *index += 1;
    tokens
        .get(*index)
        .cloned()
        .ok_or_else(|| invalid(format!("{flag} requires a value")))
}

fn set_once(slot: &mut Option<String>, value: String, flag: &str) -> Result<(), ControlError> {
    if slot.replace(value).is_some() {
        return Err(invalid(format!("{flag} may only be specified once")));
    }
    Ok(())
}

fn non_empty(value: String, label: &str) -> Result<String, ControlError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(invalid(format!("{label} cannot be empty")));
    }
    Ok(trimmed.to_string())
}

fn sanitize_source(value: String) -> Result<String, ControlError> {
    let value = non_empty(value, "--source")?;
    if value.len() > MAX_SOURCE_BYTES
        || !value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || " ._-".contains(character))
    {
        return Err(invalid(
            "Source must use at most 48 ASCII letters, numbers, spaces, `.`, `_`, or `-`",
        ));
    }
    Ok(value)
}

fn sanitize_request_id(value: String) -> Result<String, ControlError> {
    let value = non_empty(value, "--request-id")?;
    if value.len() > MAX_REQUEST_ID_BYTES
        || !value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || ".:_-".contains(character))
    {
        return Err(invalid("Request id contains unsupported characters"));
    }
    Ok(value)
}

fn invalid(message: impl Into<String>) -> ControlError {
    ControlError::new(ControlErrorCode::InvalidArguments, message)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    #[test]
    fn normal_app_launch_is_not_a_control_request() {
        let parsed = parse_app_argv(&strings(&["gitwink.exe"]), r"C:\repo").unwrap();
        assert!(parsed.is_none());
    }

    #[test]
    fn parses_commit_from_direct_or_forwarded_argv() {
        let args = strings(&[
            "gitwink.exe",
            "show",
            "commit",
            "--repo",
            ".",
            "--rev",
            "HEAD",
            "--source",
            "Codex",
            "--request-id",
            "request-1",
        ]);
        let request = parse_app_argv(&args, r"C:\repo").unwrap().unwrap();
        assert_eq!(request.request_id, "request-1");
        assert_eq!(request.source.name, "Codex");
        assert!(matches!(
            request.intent,
            ControlIntent::ShowCommit {
                revision,
                presentation: Presentation { expand: true, .. },
                ..
            } if revision == "HEAD"
        ));
    }

    #[test]
    fn file_history_defaults_repo_to_invocation_directory() {
        let request = parse_show_tokens(
            &strings(&["show", "file-history", "--path", "src/lib.rs"]),
            "/work/repo",
        )
        .unwrap();
        assert!(matches!(
            request.intent,
            ControlIntent::ShowFileHistory {
                repo: RepoLocator::LocalPath { path },
                file_path,
                ..
            } if path == "." && file_path == "src/lib.rs"
        ));
    }

    #[test]
    fn rejects_cross_target_flags_and_duplicate_values() {
        assert!(parse_show_tokens(
            &strings(&["show", "commit", "--rev", "HEAD", "--path", "a"]),
            "/repo"
        )
        .is_err());
        assert!(parse_show_tokens(
            &strings(&["show", "commit", "--rev", "HEAD", "--repo", ".", "--repo", ".."]),
            "/repo"
        )
        .is_err());
        assert!(parse_show_tokens(
            &strings(&["show", "file-history", "--path", "a", "--expand"]),
            "/repo"
        )
        .is_err());
    }
}
