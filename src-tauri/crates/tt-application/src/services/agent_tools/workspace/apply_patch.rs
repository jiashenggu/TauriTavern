use serde::Serialize;
use serde_json::{Map, Value};

use super::args::{
    classify_workspace_io_error, ensure_writable_workspace_path, optional_bool_arg,
    parse_workspace_path, required_raw_string_arg, required_trimmed_string_arg, tool_error,
};
use super::json_validation::{JsonValidationSummary, validate_json};
use super::policy::workspace_access_policy;
use crate::errors::ApplicationError;
use tt_domain::errors::{DomainError, WorkspaceWriteConflictKind};
use tt_domain::models::agent::AgentToolResult;
use tt_domain::models::tool::ToolInvocation;
use tt_domain::text_metrics::TextMetrics;
use tt_ports::repositories::workspace_repository::{WorkspaceRepository, WorkspaceWriteGuard};

use super::super::dispatcher::AgentToolEffect;
use super::super::session::AgentToolSession;
use super::super::structured::{TextMetricsPayload, structured_value};

const MAX_PATCH_EDITS: usize = 128;
const MAX_BATCH_PATCH_BYTES: usize = 8 * 1024 * 1024;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkspaceApplyPatchStructured<'a> {
    path: &'a str,
    #[serde(flatten)]
    metrics: TextMetricsPayload,
    old_sha256: &'a str,
    sha256: &'a str,
    replacements: usize,
    edit_count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    json_validation: Option<JsonValidationSummary>,
}

pub(super) struct PatchEdit<'a> {
    old_string: &'a str,
    new_string: &'a str,
    replace_all: bool,
}

type PatchArgumentError = (&'static str, String);

fn parse_edit(args: &Map<String, Value>) -> Result<PatchEdit<'_>, PatchArgumentError> {
    let old_string = required_raw_string_arg(args, "old_string").ok_or_else(|| {
        (
            "tool.invalid_arguments",
            "old_string is required".to_string(),
        )
    })?;
    let new_string = required_raw_string_arg(args, "new_string").ok_or_else(|| {
        (
            "tool.invalid_arguments",
            "new_string is required".to_string(),
        )
    })?;
    let replace_all = optional_bool_arg(args, "replace_all")
        .map_err(|message| ("tool.invalid_arguments", message))?
        .unwrap_or(false);
    if old_string.is_empty() {
        return Err((
            "workspace.patch_empty_old_string",
            "old_string cannot be empty".into(),
        ));
    }
    if old_string == new_string {
        return Err((
            "workspace.patch_no_change",
            "old_string and new_string are identical".into(),
        ));
    }
    Ok(PatchEdit {
        old_string,
        new_string,
        replace_all,
    })
}

pub(super) fn parse_edits(
    args: &Map<String, Value>,
) -> Result<Vec<PatchEdit<'_>>, PatchArgumentError> {
    let Some(value) = args.get("edits") else {
        return Ok(vec![parse_edit(args)?]);
    };
    if ["old_string", "new_string", "replace_all"]
        .iter()
        .any(|key| args.contains_key(*key))
    {
        return Err((
            "tool.invalid_arguments",
            "edits cannot be combined with top-level old_string, new_string or replace_all".into(),
        ));
    }
    let rows = value
        .as_array()
        .filter(|rows| !rows.is_empty() && rows.len() <= MAX_PATCH_EDITS)
        .ok_or_else(|| {
            (
                "tool.invalid_arguments",
                format!("edits must contain 1 to {MAX_PATCH_EDITS} replacements"),
            )
        })?;
    rows.iter().enumerate().map(|(index, row)| {
        let object = row.as_object().filter(|row| row.keys().all(|key| matches!(key.as_str(), "old_string" | "new_string" | "replace_all")))
            .ok_or_else(|| ("tool.invalid_arguments", format!("edit {} must be an object containing only old_string, new_string and optional replace_all", index + 1)))?;
        parse_edit(object).map_err(|(code, message)| (code, format!("edit {}: {message}", index + 1)))
    }).collect()
}

pub(in crate::services::agent_tools) async fn apply_patch(
    workspace_repository: &dyn WorkspaceRepository,
    run_id: &str,
    call: &ToolInvocation,
    args: &Map<String, Value>,
    session: &mut AgentToolSession,
) -> Result<(AgentToolResult, AgentToolEffect), ApplicationError> {
    let policy = workspace_access_policy(workspace_repository, run_id).await?;
    let Some(path) = required_trimmed_string_arg(args, "path") else {
        return Ok((
            tool_error(call, "tool.invalid_arguments", "path is required"),
            AgentToolEffect::None,
        ));
    };
    let json_mode = args.contains_key("json_ops");
    let edits = match if json_mode {
        Ok(Vec::new())
    } else {
        parse_edits(args)
    } {
        Ok(edits) => edits,
        Err((code, message)) => {
            return Ok((tool_error(call, code, &message), AgentToolEffect::None));
        }
    };
    let batch = json_mode || args.contains_key("edits");
    let path = match parse_workspace_path(path) {
        Ok(path) => path,
        Err(error) => return Ok((error.into_tool_result(call), AgentToolEffect::None)),
    };
    if let Err(error) = ensure_writable_workspace_path(&policy, &path) {
        return Ok((error.into_tool_result(call), AgentToolEffect::None));
    }
    let path_key = path.as_str().to_string();
    let Some(read_state) = session.read_state(&path_key).cloned() else {
        return Ok((
            tool_error(
                call,
                "workspace.patch_requires_read",
                "file must be read with workspace_read_file before applying a patch",
            ),
            AgentToolEffect::None,
        ));
    };
    if read_state.patch_requires_full_read() {
        return Ok((
            tool_error(
                call,
                "workspace.patch_requires_full_read",
                "a previous patch attempt for this file failed. Fully read the file with workspace_read_file before applying another patch.",
            ),
            AgentToolEffect::None,
        ));
    }
    let patch_uses_partial_read = !read_state.full_read;
    if batch && patch_uses_partial_read {
        session.require_full_read_before_patch(&path_key);
        return Ok((
            tool_error(
                call,
                "workspace.patch_requires_full_read",
                "fully read the file before applying an edits batch",
            ),
            AgentToolEffect::None,
        ));
    }
    let first = edits.first();
    if patch_uses_partial_read && first.is_some_and(|edit| edit.replace_all) {
        session.require_full_read_before_patch(&path_key);
        return Ok((
            tool_error(
                call,
                "workspace.patch_requires_full_read",
                "replace_all can modify text outside the range you read. Fully read the file with workspace_read_file before using replace_all.",
            ),
            AgentToolEffect::None,
        ));
    }
    if patch_uses_partial_read
        && first.is_some_and(|edit| !read_state.old_string_was_observed(edit.old_string))
    {
        session.require_full_read_before_patch(&path_key);
        return Ok((
            tool_error(
                call,
                "workspace.patch_requires_full_read",
                "old_string was not in the text you have read for this file. Fully read the file with workspace_read_file before retrying the patch.",
            ),
            AgentToolEffect::None,
        ));
    }
    let file = match workspace_repository.read_text(run_id, &path).await {
        Ok(file) => file,
        Err(error) => match classify_workspace_io_error(call, error) {
            Ok(result) => return Ok((result, AgentToolEffect::None)),
            Err(error) => return Err(error.into()),
        },
    };
    if file.sha256 != read_state.sha256 {
        if patch_uses_partial_read {
            session.require_full_read_before_patch(&path_key);
        }
        return Ok((
            tool_error(
                call,
                "workspace.patch_stale_file",
                if patch_uses_partial_read {
                    "file changed since your last read. Fully read the file with workspace_read_file before patching it."
                } else {
                    "file changed since your last full read. Read the file again before patching it."
                },
            ),
            AgentToolEffect::None,
        ));
    }
    let applied = if json_mode {
        super::json_patch::apply_json_operations(&file.text, args)
    } else {
        apply_edits_to_text(&file.text, &edits, batch, patch_uses_partial_read)
            .map(|(text, count)| (text, count, edits.len()))
    };
    let (updated, replacements, edit_count) = match applied {
        Ok(updated) => updated,
        Err((code, message)) => {
            if patch_uses_partial_read {
                session.require_full_read_before_patch(&path_key);
            }
            return Ok((tool_error(call, code, &message), AgentToolEffect::None));
        }
    };
    let mut validation_args = args.clone();
    if json_mode {
        validation_args.insert("validate_json".into(), Value::Bool(true));
    }
    let json_validation = match validate_json(&validation_args, &updated) {
        Ok(validation) => validation,
        Err((code, message)) => {
            return Ok((tool_error(call, code, &message), AgentToolEffect::None));
        }
    };
    let old_sha256 = file.sha256.clone();
    let file = match workspace_repository
        .write_text_guarded(
            run_id,
            &path,
            &updated,
            WorkspaceWriteGuard::MustMatchSha256(old_sha256.clone()),
        )
        .await
    {
        Ok(file) => file,
        Err(DomainError::WorkspaceWriteConflict { kind, .. }) => {
            if patch_uses_partial_read {
                session.require_full_read_before_patch(&path_key);
            }
            return Ok((
                patch_conflict_error(call, kind, patch_uses_partial_read),
                AgentToolEffect::None,
            ));
        }
        Err(error) => match classify_workspace_io_error(call, error) {
            Ok(result) => return Ok((result, AgentToolEffect::None)),
            Err(error) => return Err(error.into()),
        },
    };
    if patch_uses_partial_read {
        let first = first.expect("partial text patch");
        session.remember_partial_patch(&file, first.old_string, first.new_string);
    } else {
        session.remember_file(&file, true);
    }
    let metrics = TextMetrics::from_text(&file.text);
    let result = AgentToolResult {
        call_id: call.call_id.clone(),
        tool_id: call.tool_id.clone(),
        content: format!(
            "Patched {} with {} replacement(s); file now has {} chars / {} words.",
            file.path.as_str(),
            replacements,
            metrics.chars,
            metrics.words
        ),
        structured: structured_value(WorkspaceApplyPatchStructured {
            path: file.path.as_str(),
            metrics: metrics.into(),
            old_sha256: old_sha256.as_str(),
            sha256: file.sha256.as_str(),
            replacements,
            edit_count,
            json_validation,
        }),
        is_error: false,
        error_code: None,
        resource_refs: vec![file.path.as_str().to_string()],
    };
    Ok((
        result,
        AgentToolEffect::WorkspaceFilePatched {
            file,
            replacements,
            old_sha256,
        },
    ))
}

pub(super) fn apply_edits_to_text(
    text: &str,
    edits: &[PatchEdit<'_>],
    batch: bool,
    partial_read: bool,
) -> Result<(String, usize), PatchArgumentError> {
    let mut updated = text.to_string();
    let mut replacements = 0;
    for (index, edit) in edits.iter().enumerate() {
        let matches = updated.matches(edit.old_string).count();
        let failure = if matches == 0 {
            Some(("workspace.patch_old_string_not_found", if partial_read {
                "old_string was not found in the file. Fully read the file with workspace_read_file before retrying the patch."
            } else {
                "old_string was not found in the file"
            }.to_string()))
        } else if matches > 1 && !edit.replace_all {
            Some((
                "workspace.patch_old_string_not_unique",
                patch_not_unique_message(matches, partial_read),
            ))
        } else {
            None
        };
        if let Some((code, message)) = failure {
            return Err((
                code,
                if batch {
                    format!("edit {}: {message}; no edits were written", index + 1)
                } else {
                    message
                },
            ));
        }
        let count = if edit.replace_all { matches } else { 1 };
        let updated_size = updated
            .len()
            .checked_sub(count * edit.old_string.len())
            .and_then(|len| {
                count
                    .checked_mul(edit.new_string.len())
                    .and_then(|added| len.checked_add(added))
            });
        if batch && updated_size.is_none_or(|size| size > MAX_BATCH_PATCH_BYTES.max(text.len())) {
            return Err((
                "workspace.patch_output_too_large",
                format!(
                    "edit {} exceeds the batch size limit; no edits were written",
                    index + 1
                ),
            ));
        }
        updated = if edit.replace_all {
            updated.replace(edit.old_string, edit.new_string)
        } else {
            updated.replacen(edit.old_string, edit.new_string, 1)
        };
        replacements += count;
    }
    if updated == text {
        return Err((
            "workspace.patch_no_change",
            "the edits leave the file unchanged".into(),
        ));
    }
    Ok((updated, replacements))
}

fn patch_not_unique_message(matches: usize, require_full_read: bool) -> String {
    if require_full_read {
        format!(
            "old_string matched {matches} times in the file. Fully read the file with workspace_read_file before retrying with more context, or use replace_all after a full read."
        )
    } else {
        format!("old_string matched {matches} times; provide more context or set replace_all=true")
    }
}

fn patch_conflict_error(
    call: &ToolInvocation,
    kind: WorkspaceWriteConflictKind,
    require_full_read: bool,
) -> AgentToolResult {
    let changed_message = if require_full_read {
        "file changed before the patch could be written. Fully read the file with workspace_read_file before patching it."
    } else {
        "file changed before the patch could be written. Read the file again before patching it."
    };
    match kind {
        WorkspaceWriteConflictKind::AlreadyExists { .. } => {
            tool_error(call, "workspace.patch_stale_file", changed_message)
        }
        WorkspaceWriteConflictKind::Stale {
            actual_sha256: Some(_),
            ..
        } => tool_error(call, "workspace.patch_stale_file", changed_message),
        WorkspaceWriteConflictKind::Stale {
            actual_sha256: None,
            ..
        } => tool_error(
            call,
            "workspace.patch_stale_file",
            "file changed before the patch could be written and is no longer present. Read the parent directory before patching again.",
        ),
    }
}
