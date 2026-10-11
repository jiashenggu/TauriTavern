use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::HashMap;

use super::workspace_policy::InvocationWorkspaceRepository;
use crate::errors::ApplicationError;
use crate::services::agent_runtime_service::{AgentRuntimeService, PreparedInvocation};
use crate::services::agent_tools::AgentToolSession;
use crate::services::hashing::hex_lower;
use tt_domain::models::agent::WorkspacePath;
use tt_domain::models::tool::ToolInvocation;
use tt_ports::repositories::workspace_repository::{WorkspaceFile, WorkspaceRepository};

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", deny_unknown_fields)]
pub(super) enum ContextReference {
    #[serde(rename = "workspace")]
    Workspace {
        path: String,
        #[serde(default)]
        pointer: Option<String>,
    },
    #[serde(rename = "chat")]
    Chat { index: usize },
    #[serde(rename = "worldInfo")]
    WorldInfo {
        #[serde(rename = "ref")]
        reference: String,
    },
}

impl ContextReference {
    pub(super) fn tool_name(&self) -> &'static str {
        match self {
            Self::Workspace { .. } => "workspace.read_file",
            Self::Chat { .. } => "chat.read_messages",
            Self::WorldInfo { .. } => "worldinfo.read_activated",
        }
    }
}

fn invalid(message: impl Into<String>) -> ApplicationError {
    ApplicationError::ValidationError(format!(
        "agent.context_reference_invalid: {}",
        message.into()
    ))
}

impl AgentRuntimeService {
    pub(super) async fn resolve_context_references(
        &self,
        prepared: &PreparedInvocation,
        session: &AgentToolSession,
        references: &[ContextReference],
    ) -> Result<Vec<Value>, ApplicationError> {
        if references.len() > 16 {
            return Err(invalid("at most16 references are allowed"));
        }
        let repository = InvocationWorkspaceRepository::new(
            self.workspace_repository.as_ref(),
            &prepared.profile,
        );
        let roots = if references
            .iter()
            .any(|reference| matches!(reference, ContextReference::Workspace { .. }))
        {
            repository
                .read_manifest(&prepared.invocation.run_id)
                .await?
                .roots
                .into_iter()
                .filter(|root| root.visible)
                .map(|root| root.path)
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        let mut file_cache: Option<(WorkspaceFile, Option<Value>)> = None;
        let mut versions: HashMap<String, String> = HashMap::new();
        let mut results = Vec::new();
        let mut chars = 0;
        let mut reads = session.clone();
        for reference in references {
            let (tool, args) = match reference {
                ContextReference::Workspace { path, .. } => {
                    ("workspace.read_file", json!({"path":path}))
                }
                ContextReference::Chat { index } => {
                    ("chat.read_messages", json!({"messages":[{"index":index}]}))
                }
                ContextReference::WorldInfo { reference } => (
                    "worldinfo.read_activated",
                    json!({"entries":[{"ref":reference}]}),
                ),
            };
            if !prepared
                .tool_turn
                .tools()
                .iter()
                .any(|id| id.is_builtin() && id.native_name() == tool)
            {
                return Err(invalid(format!("caller must have {tool} permission")));
            }
            let (text, source) = if let ContextReference::Workspace { path, pointer } = reference {
                let path = WorkspacePath::parse(path)?;
                if !crate::services::agent_workspace_scope::workspace_path_is_under_any_root(
                    &path, &roots,
                ) {
                    return Err(invalid("workspace reference is outside caller visibility"));
                }
                let observed = session
                    .read_state(path.as_str())
                    .filter(|state| state.full_read)
                    .ok_or_else(|| {
                        invalid("workspace reference requires a prior full read by the caller")
                    })?;
                if !file_cache
                    .as_ref()
                    .is_some_and(|(file, _)| file.path == path)
                {
                    file_cache = Some((
                        repository
                            .read_text(&prepared.invocation.run_id, &path)
                            .await?,
                        None,
                    ));
                }
                let (file, parsed) = file_cache.as_mut().expect("loaded reference file");
                if file.sha256 != observed.sha256 {
                    return Err(invalid("workspace reference changed since caller read it"));
                }
                versions.insert(path.as_str().to_owned(), file.sha256.clone());
                let text = match pointer {
                    Some(pointer) => {
                        if parsed.is_none() {
                            *parsed = Some(tt_domain::json_input::from_str(&file.text).map_err(
                                |error| invalid(format!("referenced JSON rejected: {error}")),
                            )?);
                        }
                        let selected = parsed
                            .as_ref()
                            .expect("parsed reference document")
                            .pointer(pointer)
                            .ok_or_else(|| invalid("JSON Pointer does not identify a value"))?;
                        serde_json::to_string_pretty(selected)
                            .map_err(|_| invalid("cannot serialize selected JSON value"))?
                    }
                    None => file.text.clone(),
                };
                (
                    text,
                    json!({"kind":"workspace","path":path.as_str(),"pointer":pointer,"fileSha256":file.sha256}),
                )
            } else {
                let call: ToolInvocation = serde_json::from_value(json!({"callId":"context_reference","toolId":format!("builtin:{tool}"),"arguments":args}))
                    .map_err(|e| invalid(e.to_string()))?;
                let result = self
                    .tool_dispatcher
                    .dispatch_with_model_workspace_repository(
                        &prepared.invocation.run_id,
                        &call,
                        args.as_object().expect("object arguments"),
                        &mut reads,
                        &prepared.profile,
                        &repository,
                    )
                    .await?
                    .result;
                if result.is_error {
                    return Err(invalid(result.content));
                }
                let (items, text_key) = if matches!(reference, ContextReference::Chat { .. }) {
                    ("messages", "text")
                } else {
                    ("entries", "content")
                };
                let item = result
                    .structured
                    .get(items)
                    .and_then(Value::as_array)
                    .filter(|items| items.len() == 1)
                    .and_then(|items| items.first())
                    .ok_or_else(|| invalid("reference did not resolve exactly one source"))?;
                if item.get("truncated").and_then(Value::as_bool) != Some(false)
                    || item.get("lineTruncated").and_then(Value::as_bool) != Some(false)
                {
                    return Err(invalid(
                        "source exceeds full-reference read limits; provide explicitly read material instead, never a silent preview",
                    ));
                }
                let text = item
                    .get(text_key)
                    .and_then(Value::as_str)
                    .ok_or_else(|| invalid("reference has no text"))?
                    .to_owned();
                let mut metadata = item.clone();
                metadata
                    .as_object_mut()
                    .expect("source object")
                    .remove(text_key);
                metadata["kind"] = json!(if items == "messages" {
                    "chat"
                } else {
                    "worldInfo"
                });
                (text, metadata)
            };
            chars += text.chars().count();
            if chars > 96_000 {
                return Err(invalid(
                    "reference material exceeds96000 characters; reduce explicitly selected sources",
                ));
            }
            let sha256 = hex_lower(Sha256::digest(text.as_bytes()).as_slice());
            results.push(json!({"source":source,"sha256":sha256,"text":text}));
        }
        drop(file_cache);
        for (path, sha256) in versions {
            let current = repository
                .read_text(&prepared.invocation.run_id, &WorkspacePath::parse(path)?)
                .await?;
            if current.sha256 != sha256 {
                return Err(invalid(
                    "workspace reference changed while assembling the task",
                ));
            }
        }
        Ok(results)
    }
}
