use serde::Serialize;
use serde_json::{Map, Value};

use super::apply_patch::{apply_edits_to_text, parse_edits};
use super::args::{
    classify_workspace_io_error, ensure_visible_workspace_path, ensure_writable_workspace_path,
    optional_bool_arg, parse_workspace_path, required_raw_string_arg, required_trimmed_string_arg,
    tool_error,
};
use super::json_validation::{JsonValidationSummary, validate_json};
use super::policy::workspace_access_policy;
use crate::errors::ApplicationError;
use tt_domain::errors::{DomainError, WorkspaceWriteConflictKind};
use tt_domain::models::agent::{AgentToolResult, WorkspaceFileWriteMode};
use tt_domain::models::tool::ToolInvocation;
use tt_domain::text_metrics::TextMetrics;
use tt_ports::repositories::workspace_repository::WorkspaceAppendResult;
use tt_ports::repositories::workspace_repository::{WorkspaceRepository, WorkspaceWriteGuard};

use super::super::dispatcher::AgentToolEffect;
use super::super::session::AgentToolSession;
use super::super::structured::{TextMetricsPayload, structured_value};

const WRITE_MODE_INVALID_MESSAGE: &str = "mode must be `replace` or `append`";

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkspaceWriteFileStructured<'a> {
    path: &'a str,
    mode: WorkspaceFileWriteMode,
    #[serde(flatten)]
    metrics: TextMetricsPayload,
    sha256: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    source_path: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    source_sha256: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    edit_count: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    json_validation: Option<JsonValidationSummary>,
}

pub(in crate::services::agent_tools) async fn write_file(
    workspace_repository: &dyn WorkspaceRepository,
    run_id: &str,
    call: &ToolInvocation,
    args: &Map<String, Value>,
    session: &mut AgentToolSession,
) -> Result<(AgentToolResult, AgentToolEffect), ApplicationError> {
    if let Err(message) = optional_bool_arg(args, "finish_task") {
        return Ok((
            tool_error(call, "tool.invalid_arguments", &message),
            AgentToolEffect::None,
        ));
    }
    let policy = workspace_access_policy(workspace_repository, run_id).await?;
    let Some(path) = required_trimmed_string_arg(args, "path") else {
        return Ok((
            tool_error(call, "tool.invalid_arguments", "path is required"),
            AgentToolEffect::None,
        ));
    };
    if args.contains_key("content") == args.contains_key("source_path") {
        return Ok((
            tool_error(
                call,
                "tool.invalid_arguments",
                "provide exactly one of content or source_path",
            ),
            AgentToolEffect::None,
        ));
    }
    let create_only = match optional_bool_arg(args, "create_only") {
        Ok(value) => value.unwrap_or(false),
        Err(message) => {
            return Ok((
                tool_error(call, "tool.invalid_arguments", &message),
                AgentToolEffect::None,
            ));
        }
    };
    let mode = match parse_write_mode(args) {
        Ok(mode) => mode,
        Err(message) => {
            return Ok((
                tool_error(call, "workspace.write_mode_invalid", message),
                AgentToolEffect::None,
            ));
        }
    };

    let path = match parse_workspace_path(path) {
        Ok(path) => path,
        Err(error) => return Ok((error.into_tool_result(call), AgentToolEffect::None)),
    };
    if let Err(error) = ensure_writable_workspace_path(&policy, &path) {
        return Ok((error.into_tool_result(call), AgentToolEffect::None));
    }
    if mode == WorkspaceFileWriteMode::Append && (create_only || args.contains_key("source_path")) {
        return Ok((
            tool_error(
                call,
                "tool.invalid_arguments",
                "source_path and create_only require mode replace",
            ),
            AgentToolEffect::None,
        ));
    }
    let source_file = if args.contains_key("source_path") {
        let Some(source_path) = required_trimmed_string_arg(args, "source_path") else {
            return Ok((
                tool_error(
                    call,
                    "tool.invalid_arguments",
                    "source_path must be a nonempty string",
                ),
                AgentToolEffect::None,
            ));
        };
        let source = match parse_workspace_path(source_path) {
            Ok(source) => source,
            Err(error) => return Ok((error.into_tool_result(call), AgentToolEffect::None)),
        };
        if let Err(error) = ensure_visible_workspace_path(&policy, &source) {
            return Ok((error.into_tool_result(call), AgentToolEffect::None));
        }
        if source == path {
            return Ok((
                tool_error(
                    call,
                    "tool.invalid_arguments",
                    "source_path must differ from path",
                ),
                AgentToolEffect::None,
            ));
        }
        let Some(observed) = session
            .read_state(source.as_str())
            .filter(|state| state.full_read)
        else {
            return Ok((
                tool_error(
                    call,
                    "workspace.copy_requires_read",
                    "fully read the source file before copying it",
                ),
                AgentToolEffect::None,
            ));
        };
        let source_file = match workspace_repository.read_text(run_id, &source).await {
            Ok(file) => file,
            Err(error) => match classify_workspace_io_error(call, error) {
                Ok(result) => return Ok((result, AgentToolEffect::None)),
                Err(error) => return Err(error.into()),
            },
        };
        if source_file.sha256 != observed.sha256 {
            return Ok((
                tool_error(
                    call,
                    "workspace.copy_stale_source",
                    "source changed since your last full read or write; fully read it again before copying",
                ),
                AgentToolEffect::None,
            ));
        }
        Some(source_file)
    } else {
        None
    };
    let json_mode = args.contains_key("json_ops");
    let edited = if args.contains_key("edits") || json_mode {
        let Some(source) = &source_file else {
            return Ok((
                tool_error(
                    call,
                    "tool.invalid_arguments",
                    "edits and json_ops require source_path",
                ),
                AgentToolEffect::None,
            ));
        };
        let updated = if json_mode {
            super::json_patch::apply_json_operations(&source.text, args).map(|(text, _, _)| text)
        } else {
            parse_edits(args).and_then(|edits| {
                apply_edits_to_text(&source.text, &edits, true, false).map(|(text, _)| text)
            })
        };
        match updated {
            Ok(text) => Some(text),
            Err((code, message)) => {
                return Ok((tool_error(call, code, &message), AgentToolEffect::None));
            }
        }
    } else {
        None
    };
    let content = match (&edited, &source_file) {
        (Some(text), _) => text.as_str(),
        (None, Some(file)) => file.text.as_str(),
        (None, None) => match required_raw_string_arg(args, "content") {
            Some(content) => content,
            None => {
                return Ok((
                    tool_error(call, "tool.invalid_arguments", "content must be a string"),
                    AgentToolEffect::None,
                ));
            }
        },
    };
    let mut validation_args = args.clone();
    if json_mode {
        validation_args.insert("validate_json".into(), Value::Bool(true));
    }
    let json_validation = match validate_json(&validation_args, content) {
        Ok(validation) => validation,
        Err((code, message)) => {
            return Ok((tool_error(call, code, &message), AgentToolEffect::None));
        }
    };
    if mode == WorkspaceFileWriteMode::Append && json_validation.is_some() {
        return Ok((
            tool_error(
                call,
                "tool.invalid_arguments",
                "validate_json is only supported for complete replace writes",
            ),
            AgentToolEffect::None,
        ));
    }
    let (file, file_is_fully_known) = match mode {
        WorkspaceFileWriteMode::Replace => {
            let write_guard = if create_only {
                WorkspaceWriteGuard::MustNotExist
            } else {
                match workspace_repository.read_text(run_id, &path).await {
                    Ok(current) => {
                        let Some(read_state) = session.read_state(path.as_str()) else {
                            return Ok((
                                tool_error(
                                    call,
                                    "workspace.write_requires_read",
                                    "file already exists; read it with workspace_read_file before rewriting it",
                                ),
                                AgentToolEffect::None,
                            ));
                        };
                        if current.sha256 != read_state.sha256 {
                            return Ok((
                                tool_error(
                                    call,
                                    "workspace.write_stale_file",
                                    "file changed since you last read or wrote it. Read the file again before rewriting it.",
                                ),
                                AgentToolEffect::None,
                            ));
                        }
                        WorkspaceWriteGuard::MustMatchSha256(read_state.sha256.clone())
                    }
                    Err(DomainError::NotFound(_)) => WorkspaceWriteGuard::MustNotExist,
                    Err(error) => match classify_workspace_io_error(call, error) {
                        Ok(result) => return Ok((result, AgentToolEffect::None)),
                        Err(error) => return Err(error.into()),
                    },
                }
            };
            match workspace_repository
                .write_text_guarded(run_id, &path, content, write_guard)
                .await
            {
                Ok(file) => (file, true),
                Err(DomainError::WorkspaceWriteConflict { kind, .. }) => {
                    if create_only
                        && matches!(&kind, WorkspaceWriteConflictKind::AlreadyExists { .. })
                    {
                        return Ok((
                            tool_error(
                                call,
                                "workspace.write_target_exists",
                                "create_only requires a new target; the existing file was not changed",
                            ),
                            AgentToolEffect::None,
                        ));
                    }
                    return Ok((write_conflict_error(call, kind), AgentToolEffect::None));
                }
                Err(error) => match classify_workspace_io_error(call, error) {
                    Ok(result) => return Ok((result, AgentToolEffect::None)),
                    Err(error) => return Err(error.into()),
                },
            }
        }
        WorkspaceFileWriteMode::Append => {
            let result = match workspace_repository
                .append_text(run_id, &path, content)
                .await
            {
                Ok(result) => result,
                Err(error) => match classify_workspace_io_error(call, error) {
                    Ok(result) => return Ok((result, AgentToolEffect::None)),
                    Err(error) => return Err(error.into()),
                },
            };
            let file_is_fully_known = appended_file_is_fully_known(session, path.as_str(), &result);
            (result.file, file_is_fully_known)
        }
    };
    if file_is_fully_known {
        session.remember_file(&file, true);
    }
    let metrics = TextMetrics::from_text(&file.text);

    let result = AgentToolResult {
        call_id: call.call_id.clone(),
        tool_id: call.tool_id.clone(),
        content: format!(
            "{} {} chars / {} words to {}.",
            write_mode_past_tense(mode),
            metrics.chars,
            metrics.words,
            file.path.as_str()
        ),
        structured: structured_value(WorkspaceWriteFileStructured {
            path: file.path.as_str(),
            mode,
            metrics: metrics.into(),
            sha256: file.sha256.as_str(),
            source_path: source_file.as_ref().map(|source| source.path.as_str()),
            source_sha256: source_file.as_ref().map(|source| source.sha256.as_str()),
            edit_count: args
                .get(if json_mode { "json_ops" } else { "edits" })
                .and_then(Value::as_array)
                .map(Vec::len),
            json_validation,
        }),
        is_error: false,
        error_code: None,
        resource_refs: vec![file.path.as_str().to_string()],
    };

    Ok((result, AgentToolEffect::WorkspaceFileWritten { file, mode }))
}

fn parse_write_mode(args: &Map<String, Value>) -> Result<WorkspaceFileWriteMode, &'static str> {
    let Some(value) = args.get("mode") else {
        return Ok(WorkspaceFileWriteMode::Replace);
    };
    let Some(raw) = value.as_str() else {
        return Err(WRITE_MODE_INVALID_MESSAGE);
    };
    match raw.trim() {
        "replace" => Ok(WorkspaceFileWriteMode::Replace),
        "append" => Ok(WorkspaceFileWriteMode::Append),
        _ => Err(WRITE_MODE_INVALID_MESSAGE),
    }
}

fn appended_file_is_fully_known(
    session: &AgentToolSession,
    path: &str,
    result: &WorkspaceAppendResult,
) -> bool {
    match result.previous_sha256.as_deref() {
        None => true,
        Some(previous_sha256) => session
            .read_state(path)
            .is_some_and(|state| state.full_read && state.sha256 == previous_sha256),
    }
}

fn write_mode_past_tense(mode: WorkspaceFileWriteMode) -> &'static str {
    match mode {
        WorkspaceFileWriteMode::Replace => "Wrote",
        WorkspaceFileWriteMode::Append => "Appended",
    }
}

fn write_conflict_error(
    call: &ToolInvocation,
    kind: WorkspaceWriteConflictKind,
) -> AgentToolResult {
    match kind {
        WorkspaceWriteConflictKind::AlreadyExists { .. } => tool_error(
            call,
            "workspace.write_requires_read",
            "file already exists; read it with workspace_read_file before rewriting it",
        ),
        WorkspaceWriteConflictKind::Stale {
            actual_sha256: Some(_),
            ..
        } => tool_error(
            call,
            "workspace.write_stale_file",
            "file changed since you last read or wrote it. Read the file again before rewriting it.",
        ),
        WorkspaceWriteConflictKind::Stale {
            actual_sha256: None,
            ..
        } => tool_error(
            call,
            "workspace.write_stale_file",
            "file changed since you last read or wrote it and is no longer present. Read the parent directory before writing again.",
        ),
    }
}
