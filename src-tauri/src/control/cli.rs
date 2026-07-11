use std::ffi::OsString;
use std::path::PathBuf;
use std::process::Command;

use serde_json::json;

use super::model::{ControlIntent, ControlRequest, RepoLocator, PROTOCOL_ID};
use super::parser;

const HELP: &str = r#"gitwinkctl — open precise Git evidence in gitwink

USAGE
  gitwinkctl capabilities [--json]
  gitwinkctl show commit [--repo <path>] --rev <revision> [--no-expand] [--source <name>] [--request-id <id>] [--json]
  gitwinkctl show file-history [--repo <path>] --path <repo-relative-file> [--source <name>] [--request-id <id>] [--json]
  gitwinkctl help [agents]
  gitwinkctl version

BEHAVIOR
  --repo defaults to the current working directory and may point at any
  subdirectory of a registered working tree. Show commands are read-only.
  The newest request wins when several arrive together.

EXAMPLES
  gitwinkctl show commit --repo . --rev HEAD
  gitwinkctl show commit --repo A:\projects\gitwink --rev fc78436 --source Codex
  gitwinkctl show file-history --repo . --path src/App.tsx
  gitwinkctl capabilities --json

Run `gitwinkctl help agents` for machine-integration guidance.
"#;

const AGENT_HELP: &str = r#"gitwink agent integration

1. Run `gitwinkctl capabilities --json` before relying on an action.
2. Use `show commit` for a committed result and `show file-history` when the
   user explicitly asks to open or trace one file's history.
3. Prefer full commit OIDs. Short revisions and refs are accepted only when
   the local repository resolves them unambiguously.
4. File paths must be repository-relative. Never auto-add, restore, clone, or
   fetch a repository to satisfy a show request.
5. A successful CLI response is `queued`, not proof that the webview rendered
   the target. Structured end-to-end acknowledgements are a later capability.
6. Reuse `--request-id` only when retrying the same visual handoff.
7. If gitwinkctl is unavailable, fall back to a concise textual explanation;
   gitwink integration is optional and must never be required for correctness.
"#;

pub fn run() -> i32 {
    let mut args: Vec<String> = std::env::args_os()
        .skip(1)
        .map(|argument| argument.to_string_lossy().into_owned())
        .collect();
    if args.is_empty()
        || matches!(
            args.first().map(String::as_str),
            Some("help" | "--help" | "-h")
        )
    {
        match args.get(1).map(String::as_str) {
            None => print!("{HELP}"),
            Some("agents") if args.len() == 2 => print!("{AGENT_HELP}"),
            Some(topic) => {
                eprintln!("Unknown help topic `{topic}`\n\n{HELP}");
                return 2;
            }
        }
        return 0;
    }

    match args[0].as_str() {
        "capabilities" => match optional_json_flag(&args[1..]) {
            Ok(json_output) => print_capabilities(json_output),
            Err(message) => print_error("invalid_arguments", message, false, 2),
        },
        "version" | "--version" | "-V" => {
            if args.len() != 1 {
                return print_error(
                    "invalid_arguments",
                    "The version command does not accept arguments",
                    false,
                    2,
                );
            }
            println!("gitwinkctl {}", env!("CARGO_PKG_VERSION"));
            0
        }
        "show" => {
            if args
                .iter()
                .filter(|argument| argument.as_str() == "--json")
                .count()
                > 1
            {
                return print_error(
                    "invalid_arguments",
                    "`--json` may only be specified once",
                    true,
                    2,
                );
            }
            let json_output = remove_flag(&mut args, "--json");
            dispatch_show(args, json_output)
        }
        unknown => {
            eprintln!("Unknown command `{unknown}`\n\n{HELP}");
            2
        }
    }
}

fn optional_json_flag(arguments: &[String]) -> Result<bool, &'static str> {
    match arguments {
        [] => Ok(false),
        [only] if only == "--json" => Ok(true),
        [first, second, ..] if first == "--json" && second == "--json" => {
            Err("`--json` may only be specified once")
        }
        _ => Err("Only `--json` is accepted by this command"),
    }
}

fn print_capabilities(json_output: bool) -> i32 {
    let value = json!({
        "protocol": PROTOCOL_ID,
        "version": env!("CARGO_PKG_VERSION"),
        "command": "gitwinkctl",
        "activationCommand": "gitwink show",
        "guiBinaryAcceptsShowArguments": true,
        "readOnly": true,
        "constraints": {
            "repositoryMustBeRegistered": true,
            "repositoryMustBeActive": true,
            "autoAddRepository": false,
            "resultSemantics": "queued",
            "exitCodes": { "queued": 0, "invalidArguments": 2, "launchFailure": 3 }
        },
        "delivery": {
            "transport": "single_instance_argv",
            "latestWins": true,
            "structuredAcknowledgement": false
        },
        "actions": [
            {
                "name": "show_commit",
                "command": "show commit",
                "required": ["revision"],
                "optional": ["repo", "expand", "source", "requestId"],
                "exactResolution": true
            },
            {
                "name": "show_file_history",
                "command": "show file-history",
                "required": ["filePath"],
                "optional": ["repo", "source", "requestId"],
                "filePathKind": "repo_relative",
                "followsRenames": false
            }
        ],
        "plannedNotAvailable": [
            "inspect",
            "show_range",
            "show_worktree",
            "view_links",
            "portable_repo_locator",
            "render_acknowledgement"
        ]
    });
    if json_output {
        println!("{}", serde_json::to_string_pretty(&value).unwrap());
    } else {
        println!(
            "gitwink control {}\n\nAvailable:\n  show commit\n  show file-history\n\nRun `gitwinkctl help agents` for integration guidance.",
            env!("CARGO_PKG_VERSION")
        );
    }
    0
}

fn dispatch_show(tokens: Vec<String>, json_output: bool) -> i32 {
    let cwd = match std::env::current_dir() {
        Ok(path) => path.to_string_lossy().into_owned(),
        Err(error) => return print_error("cwd_unavailable", &error.to_string(), json_output, 3),
    };
    let request = match parser::parse_show_tokens(&tokens, &cwd) {
        Ok(request) => request,
        Err(error) => return print_error("invalid_arguments", &error.message, json_output, 2),
    };
    let app = match find_app_executable() {
        Some(path) => path,
        None => {
            return print_error(
                "app_not_found",
                "Could not find the gitwink app beside gitwinkctl or on PATH; set GITWINK_APP to its executable path",
                json_output,
                3,
            )
        }
    };
    let args = request_to_app_args(&request);
    if let Err(error) = Command::new(&app)
        .args(&args)
        .current_dir(&request.invocation_cwd)
        .spawn()
    {
        return print_error(
            "dispatch_failed",
            &format!("Could not launch `{}`: {error}", app.display()),
            json_output,
            3,
        );
    }

    let target = target_description(&request);
    if json_output {
        println!(
            "{}",
            serde_json::to_string(&json!({
                "protocol": PROTOCOL_ID,
                "status": "queued",
                "requestId": request.request_id,
                "target": target,
                "acknowledged": false
            }))
            .unwrap()
        );
    } else {
        println!("Queued {target} in gitwink.");
    }
    0
}

fn request_to_app_args(request: &ControlRequest) -> Vec<OsString> {
    let mut args = vec![OsString::from("show")];
    match &request.intent {
        ControlIntent::ShowCommit {
            repo,
            revision,
            presentation,
        } => {
            args.push("commit".into());
            push_repo(&mut args, repo);
            args.extend(["--rev".into(), revision.into()]);
            if !presentation.expand {
                args.push("--no-expand".into());
            }
        }
        ControlIntent::ShowFileHistory {
            repo, file_path, ..
        } => {
            args.push("file-history".into());
            push_repo(&mut args, repo);
            args.extend(["--path".into(), file_path.into()]);
        }
    }
    args.extend([
        "--source".into(),
        request.source.name.as_str().into(),
        "--request-id".into(),
        request.request_id.as_str().into(),
    ]);
    args
}

fn push_repo(args: &mut Vec<OsString>, repo: &RepoLocator) {
    let RepoLocator::LocalPath { path } = repo;
    args.extend(["--repo".into(), path.into()]);
}

fn target_description(request: &ControlRequest) -> String {
    match &request.intent {
        ControlIntent::ShowCommit { revision, .. } => format!("commit {revision}"),
        ControlIntent::ShowFileHistory { file_path, .. } => {
            format!("file history for {file_path}")
        }
    }
}

fn find_app_executable() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("GITWINK_APP").map(PathBuf::from) {
        return path.is_file().then_some(path);
    }
    if let Ok(current) = std::env::current_exe() {
        let sibling = current.with_file_name(if cfg!(windows) {
            "gitwink.exe"
        } else {
            "gitwink"
        });
        if sibling.is_file() {
            return Some(sibling);
        }
    }
    Some(PathBuf::from(if cfg!(windows) {
        "gitwink.exe"
    } else {
        "gitwink"
    }))
}

fn remove_flag(args: &mut Vec<String>, flag: &str) -> bool {
    let before = args.len();
    args.retain(|argument| argument != flag);
    before != args.len()
}

fn print_error(code: &str, message: &str, json_output: bool, exit_code: i32) -> i32 {
    if json_output {
        eprintln!(
            "{}",
            serde_json::to_string(&json!({
                "protocol": PROTOCOL_ID,
                "status": "error",
                "error": { "code": code, "message": message }
            }))
            .unwrap()
        );
    } else {
        eprintln!("gitwinkctl: {message}");
    }
    exit_code
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forwarded_arguments_preserve_request_identity_and_target() {
        let request = parser::parse_show_tokens(
            &[
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
            ]
            .map(str::to_string),
            "/repo",
        )
        .unwrap();
        let args = request_to_app_args(&request)
            .into_iter()
            .map(|value| value.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert_eq!(
            args,
            [
                "show",
                "commit",
                "--repo",
                ".",
                "--rev",
                "HEAD",
                "--source",
                "Codex",
                "--request-id",
                "request-1"
            ]
        );
    }

    #[test]
    fn optional_json_flag_is_strict() {
        assert_eq!(optional_json_flag(&[]), Ok(false));
        assert_eq!(optional_json_flag(&["--json".to_string()]), Ok(true));
        assert!(optional_json_flag(&["--other".to_string()]).is_err());
        assert!(optional_json_flag(&["--json".to_string(), "--json".to_string()]).is_err());
    }
}
