use super::{AgentRuntimeService, PreparedInvocation};
use crate::errors::ApplicationError;
use crate::services::agent_tools::{AgentToolDispatchOutcome, AgentToolEffect, AgentToolSession};
use serde_json::json;
use tt_domain::models::agent::{AgentChatCommitMode, AgentRunEventLevel, WorkspacePath};

impl AgentRuntimeService {
    pub(super) async fn reviewed_body_sha(
        &self,
        prepared: &PreparedInvocation,
        session: &AgentToolSession,
    ) -> Result<String, ApplicationError> {
        let path = WorkspacePath::parse(&prepared.profile.output.message_body_path)?;
        let known = session.read_state(path.as_str()).filter(|read| read.full_read)
            .ok_or_else(|| ApplicationError::ValidationError("agent.finalize_body_unread: fully read or adopt the reviewed body before finalizing state".into()))?;
        if self
            .has_pending_child_tasks(&prepared.invocation.run_id, &prepared.invocation.id)
            .await?
        {
            return Err(ApplicationError::ValidationError(
                "agent.finalize_children_pending: collect outstanding tasks before finalizing"
                    .into(),
            ));
        }
        let file = self
            .workspace_repository
            .read_text(&prepared.invocation.run_id, &path)
            .await?;
        if file.sha256 != known.sha256 {
            return Err(ApplicationError::ValidationError(
                "agent.finalize_body_changed: body changed since the last read or adoption".into(),
            ));
        }
        Ok(file.sha256)
    }

    pub(super) async fn finalize_successful_mutation(
        &self,
        prepared: &PreparedInvocation,
        mut outcome: AgentToolDispatchOutcome,
        expected_body_sha: &str,
    ) -> Result<AgentToolDispatchOutcome, ApplicationError> {
        let (event_type, file, extra) = match &outcome.effect {
            AgentToolEffect::WorkspaceFileWritten { file, mode } => {
                ("workspace_file_written", file, json!({"mode":mode}))
            }
            AgentToolEffect::WorkspaceFilePatched {
                file,
                replacements,
                old_sha256,
            } => (
                "workspace_patch_applied",
                file,
                json!({"replacements":replacements,"oldSha256":old_sha256}),
            ),
            _ => return Err(ApplicationError::InternalError(
                "agent.finalize_mutation_missing: successful state operation has no file effect"
                    .into(),
            )),
        };
        let mut payload = extra;
        payload["invocationId"] = json!(prepared.invocation.id);
        payload["path"] = json!(file.path.as_str());
        payload["sha256"] = json!(file.sha256);
        payload["chars"] = json!(file.text.chars().count());
        self.event(
            &prepared.invocation.run_id,
            AgentRunEventLevel::Info,
            event_type,
            payload,
        )
        .await?;
        let path = WorkspacePath::parse(&prepared.profile.output.message_body_path)?;
        let current = self
            .workspace_repository
            .read_text(&prepared.invocation.run_id, &path)
            .await?;
        if current.sha256 != expected_body_sha {
            outcome.result.is_error = true;
            outcome.result.error_code = Some("agent.finalize_body_changed".into());
            outcome.result.content = "State file was saved, but the body changed before publication. Review the current body and retry commit; do not repeat the state operation.".into();
            outcome.effect = AgentToolEffect::None;
        } else {
            outcome.effect = AgentToolEffect::ChatCommitRequested {
                path,
                mode: AgentChatCommitMode::Replace,
                reason: Some("Reviewed body and validated state ready".into()),
                finish: true,
            };
        }
        Ok(outcome)
    }
}
