use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::AgentRuntimeService;
use crate::dto::agent_dto::{
    AgentSaveStateDto, AgentSaveStateResultDto, AgentStateDocumentDto, AgentStateFieldDto,
    AgentStateTargetDto,
};
use crate::errors::ApplicationError;
use crate::services::agent_identity::{validate_stable_chat_id, workspace_id_for_stable_chat_id};
use crate::services::agent_workspace_lifecycle_service::AgentRunActivity;
use tt_domain::models::agent::{AgentChatRef, WorkspacePath};
use tt_ports::repositories::chat_payload_commit_repository::{
    ChatMessageSnapshot, ChatPayloadTarget,
};
use tt_ports::repositories::workspace_repository::WorkspaceFile;

const MAX_STATE_BYTES: usize = 8 * 1024 * 1024;

fn invalid(message: impl std::fmt::Display) -> ApplicationError {
    ApplicationError::ValidationError(format!("agent.state_invalid: {message}"))
}

fn conflict() -> ApplicationError {
    ApplicationError::Conflict(
        "agent.state_conflict: chat, swipe or state changed; reload before saving".into(),
    )
}

fn chat_target(target: &AgentStateTargetDto) -> ChatPayloadTarget {
    match &target.chat_ref {
        AgentChatRef::Character {
            character_id,
            file_name,
        } => ChatPayloadTarget::Character {
            character_id: character_id.clone(),
            file_name: file_name.clone(),
        },
        AgentChatRef::Group { chat_id } => ChatPayloadTarget::Group {
            chat_id: chat_id.clone(),
        },
    }
}

impl AgentRuntimeService {
    async fn selected_state(
        &self,
        target: &AgentStateTargetDto,
    ) -> Result<(String, String, ChatMessageSnapshot), ApplicationError> {
        let stable = validate_stable_chat_id(&target.stable_chat_id)?;
        let snapshot = self
            .chat_commit_repository
            .read_message_snapshot(chat_target(target), target.message_index)
            .await?;
        let identity_matches = match &target.chat_ref {
            AgentChatRef::Character { .. } => {
                snapshot
                    .chat_metadata
                    .get("integrity")
                    .and_then(Value::as_str)
                    == Some(stable.as_str())
            }
            AgentChatRef::Group { chat_id } => chat_id == &stable,
        };
        let message = &snapshot.message;
        let swipe = message
            .get("swipe_id")
            .and_then(Value::as_u64)
            .map(|value| value as usize);
        if !identity_matches
            || swipe != target.swipe_id
            || message.get("is_user").and_then(Value::as_bool) == Some(true)
            || message.get("is_system").and_then(Value::as_bool) == Some(true)
        {
            return Err(conflict());
        }
        let metadata = message
            .pointer("/extra/tauritavern/agent")
            .ok_or_else(|| invalid("selected message has no Agent state"))?;
        if metadata.get("persistStateStatus").and_then(Value::as_str) != Some("committed") {
            return Err(invalid(
                "persistent state is not published; finish or resume the run first",
            ));
        }
        let state_id = metadata
            .get("persistStateId")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid("missing state ID"))?
            .to_owned();
        let workspace = workspace_id_for_stable_chat_id(&target.chat_ref, &stable)?;
        Ok((workspace, state_id, snapshot))
    }

    pub(super) async fn adopt_manual_state_for_revision(
        &self,
        run: &tt_domain::models::agent::AgentRun,
        chat_ref: &AgentChatRef,
    ) -> Result<Option<String>, ApplicationError> {
        if !self
            .active_run_ids_for_workspace(&run.workspace_id)
            .await?
            .is_empty()
        {
            return Err(invalid("cannot revise while another run is active"));
        }
        let Some(message) = self.find_last_chat_message(chat_ref).await? else {
            return Ok(None);
        };
        let Some(metadata) = message.message.pointer("/extra/tauritavern/agent") else {
            return Ok(None);
        };
        if metadata.get("runId").and_then(Value::as_str) != Some(run.id.as_str())
            || metadata.get("manualStateEdit").is_none()
        {
            return Ok(None);
        }
        if metadata.get("persistStateStatus").and_then(Value::as_str) != Some("committed") {
            return Err(invalid("cannot revise an unpublished state"));
        }
        let state_id = metadata
            .get("persistStateId")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid("missing revision state"))?;
        let path = WorkspacePath::parse("persist/state.json")?;
        let file = self
            .workspace_repository
            .read_persistent_file(&run.workspace_id, state_id, &path)
            .await?;
        self.workspace_repository
            .write_text(&run.id, &path, &file.text)
            .await?;
        Ok(Some(state_id.to_owned()))
    }

    pub async fn read_state_document(
        &self,
        target: AgentStateTargetDto,
    ) -> Result<AgentStateDocumentDto, ApplicationError> {
        let _lock = self.run_lifecycle_lock.lock().await;
        let (workspace, state_id, snapshot) = self.selected_state(&target).await?;
        let file = self
            .workspace_repository
            .read_persistent_file(
                &workspace,
                &state_id,
                &WorkspacePath::parse("persist/state.json")?,
            )
            .await?;
        let document = parse_state(&file.text)?;
        let mut fields = Vec::new();
        collect_fields(&document, "", &mut fields)?;
        Ok(AgentStateDocumentDto {
            state_id,
            sha256: file.sha256,
            chat_sha256: snapshot.chat_sha256,
            text: serde_json::to_string_pretty(&document).map_err(invalid)?,
            fields,
            manual_edit: snapshot
                .message
                .pointer("/extra/tauritavern/agent/manualStateEdit")
                .cloned(),
            read_only: !self
                .active_run_ids_for_workspace(&workspace)
                .await?
                .is_empty(),
        })
    }

    pub async fn save_state_document(
        &self,
        dto: AgentSaveStateDto,
    ) -> Result<AgentSaveStateResultDto, ApplicationError> {
        let _lock = self.run_lifecycle_lock.lock().await;
        uuid::Uuid::parse_str(&dto.edit_id).map_err(invalid)?;
        let (workspace, state_id, mut snapshot) = self.selected_state(&dto.target).await?;
        if snapshot
            .message
            .pointer("/extra/tauritavern/agent/manualStateEdit/editId")
            .and_then(Value::as_str)
            == Some(dto.edit_id.as_str())
        {
            return Ok(AgentSaveStateResultDto {
                state_id,
                agent_metadata: snapshot.message["extra"]["tauritavern"]["agent"].clone(),
            });
        }
        if !self
            .active_run_ids_for_workspace(&workspace)
            .await?
            .is_empty()
        {
            return Err(invalid("workspace has an active run; state is read-only"));
        }
        if state_id != dto.expected_state_id || snapshot.chat_sha256 != dto.expected_chat_sha256 {
            return Err(conflict());
        }
        let path = WorkspacePath::parse("persist/state.json")?;
        let source = self
            .workspace_repository
            .read_persistent_file(&workspace, &state_id, &path)
            .await?;
        if source.sha256 != dto.expected_sha256 {
            return Err(conflict());
        }
        let original = parse_state(&source.text)?;
        let mut updated = match (dto.text, dto.edits) {
            (Some(text), None) => parse_state(&text)?,
            (None, Some(edits)) if edits.len() <= 4096 => {
                let bytes = edits
                    .iter()
                    .try_fold(0usize, |size, edit| size.checked_add(edit.value_json.len()));
                if bytes.is_none_or(|size| size > MAX_STATE_BYTES)
                    || edits.iter().any(|edit| edit.pointer.len() > 4096)
                {
                    return Err(invalid("field edits exceed the size or path limit"));
                }
                let mut updated = original.clone();
                for edit in edits {
                    let value = tt_domain::json_input::from_str::<Value>(&edit.value_json)
                        .map_err(invalid)?;
                    let destination = updated
                        .pointer_mut(&edit.pointer)
                        .ok_or_else(|| invalid("field no longer exists"))?;
                    if value_kind(destination) != edit.kind || value_kind(&value) != edit.kind {
                        return Err(invalid(
                            "field type changed; use the JSON editor for structural changes",
                        ));
                    }
                    *destination = value;
                }
                updated
            }
            _ => return Err(invalid("provide either JSON text or field edits")),
        };
        if updated.get("schemaVersion") != original.get("schemaVersion")
            || updated.get("revision") != original.get("revision")
        {
            return Err(invalid("schemaVersion and revision are managed fields"));
        }
        if updated == original {
            return Err(invalid("no changes to save"));
        }
        if let Some(revision) = original.get("revision") {
            let revision = revision
                .as_u64()
                .and_then(|value| value.checked_add(1))
                .ok_or_else(|| invalid("invalid revision"))?;
            updated["revision"] = json!(revision);
        }
        let text = serde_json::to_string_pretty(&updated).map_err(invalid)?;
        parse_state(&text)?;
        collect_fields(&updated, "", &mut Vec::new())?;
        let file = WorkspaceFile {
            path,
            bytes: text.len() as u64,
            sha256: crate::services::hashing::hex_lower(Sha256::digest(text.as_bytes()).as_slice()),
            text,
        };
        let new_id = self
            .workspace_repository
            .fork_persistent_file(&workspace, &state_id, &file, &dto.expected_sha256)
            .await?;
        let agent = snapshot
            .message
            .pointer_mut("/extra/tauritavern/agent")
            .and_then(Value::as_object_mut)
            .ok_or_else(|| invalid("invalid Agent metadata"))?;
        agent.insert("persistStateId".into(), json!(new_id));
        agent.insert("manualStateEdit".into(), json!({"editId":dto.edit_id,"baseStateId":state_id,"stateId":new_id,"editedAt":chrono::Utc::now()}));
        let agent_metadata = Value::Object(agent.clone());
        let extra = snapshot.message["extra"].clone();
        if let Some(swipe) = dto.target.swipe_id
            && let Some(info) = snapshot
                .message
                .get_mut("swipe_info")
                .and_then(Value::as_array_mut)
        {
            let selected = info
                .get_mut(swipe)
                .and_then(Value::as_object_mut)
                .ok_or_else(|| invalid("invalid active swipe metadata"))?;
            selected.insert("extra".into(), extra);
        }
        self.chat_commit_repository
            .replace_message_guarded(
                chat_target(&dto.target),
                dto.target.message_index,
                &snapshot.chat_sha256,
                snapshot.message,
            )
            .await?;
        Ok(AgentSaveStateResultDto {
            state_id: new_id,
            agent_metadata,
        })
    }
}

fn parse_state(text: &str) -> Result<Value, ApplicationError> {
    if text.len() > MAX_STATE_BYTES {
        return Err(invalid("state exceeds 8 MiB"));
    }
    let value: Value = tt_domain::json_input::from_str(text).map_err(invalid)?;
    if !value.is_object() {
        return Err(invalid("state must be a JSON object"));
    }
    Ok(value)
}

fn value_kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn collect_fields(
    value: &Value,
    pointer: &str,
    fields: &mut Vec<AgentStateFieldDto>,
) -> Result<(), ApplicationError> {
    if fields.len() >= 4096 || pointer.len() > 4096 {
        return Err(invalid(
            "state exceeds the editor field count or path length limit",
        ));
    }
    match value {
        Value::Object(map) if !map.is_empty() => {
            for (key, value) in map {
                collect_fields(
                    value,
                    &format!("{pointer}/{}", key.replace('~', "~0").replace('/', "~1")),
                    fields,
                )?;
            }
        }
        Value::Array(items) if !items.is_empty() => {
            for (index, value) in items.iter().enumerate() {
                collect_fields(value, &format!("{pointer}/{index}"), fields)?;
            }
        }
        _ => fields.push(AgentStateFieldDto {
            pointer: pointer.into(),
            kind: value_kind(value).into(),
            value_json: value.to_string(),
        }),
    }
    Ok(())
}
