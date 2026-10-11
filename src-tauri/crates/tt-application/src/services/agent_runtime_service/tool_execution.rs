use std::time::Instant;

use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;

use super::commit_ledger::RunCommitLedger;
use super::delegation::workspace_policy::InvocationWorkspaceRepository;
use super::markdown::render_markdown_value;
use super::model_stream_projection::remove_live_tool_call;
use super::{AgentRuntimeService, PreparedInvocation};
use crate::errors::ApplicationError;
use crate::services::hashing::hex_lower;
use crate::services::tool_request_gate::{ToolRequestGate, ToolRequestGateError};

use crate::services::agent_tools::{
    AGENT_AWAIT, AGENT_DELEGATE, AGENT_HANDOFF, AGENT_LIST, AgentToolDispatchOutcome,
    AgentToolEffect, AgentToolSession, TASK_RETURN, WORKSPACE_FINISH,
};
use tt_domain::models::agent::{
    AgentInvocationExitPolicy, AgentRunEventLevel, AgentRunPresentation, AgentRunStatus,
    AgentToolResult, WorkspacePath,
};
use tt_domain::models::tool::{InvocationToolSnapshot, ToolId, ToolInvocation};
use tt_domain::text_metrics::TextMetrics;
use tt_ports::mcp::{McpCallOutcome, McpKnownResponse};
use tt_ports::repositories::workspace_repository::WorkspaceWriteGuard;

const TOOL_CALL_AUDIT_DIGEST_BYTES: usize = 8;
const MCP_RESULT_CONTENT_CHUNK_CHARS: usize = 3_000;

pub(super) struct ToolCallFailure {
    pub error: ApplicationError,
    pub started: bool,
}

impl AgentRuntimeService {
    #[expect(
        clippy::too_many_arguments,
        reason = "tool dispatch boundary keeps invocation, call position, session, ledger, and cancellation explicit"
    )]
    pub(super) async fn dispatch_tool_call(
        &self,
        prepared: &PreparedInvocation,
        round: usize,
        tool_call_index: usize,
        tool_invocation: &ToolInvocation,
        gate: &mut ToolRequestGate,
        session: &mut AgentToolSession,
        is_last_call: bool,
        commit_ledger: &mut RunCommitLedger,
        cancel: &mut super::AgentCancelReceiver,
    ) -> Result<AgentToolDispatchOutcome, ToolCallFailure> {
        let mut started_tool = false;
        let result = async {
            let run_id = prepared.invocation.run_id.as_str();
            let invocation_id = prepared.invocation.id.as_str();
            let exit_policy = prepared.invocation.exit_policy;
            let profile = &prepared.profile;
            let tool_name = tool_invocation.tool_id.native_name();
            let snapshot_id = prepared.tool_snapshot.id().as_str();
            let arguments_ref = self.store_tool_arguments(run_id, invocation_id, round, tool_invocation).await?;
            self.event(
                run_id,
                AgentRunEventLevel::Info,
                "tool_call_requested",
                json!({
                    "round": round,
                    "invocationId": invocation_id,
                    "callId": tool_invocation.call_id.as_str(),
                    "toolId": tool_invocation.tool_id.as_str(),
                    "snapshotId": snapshot_id,
                    "name": tool_name,
                    "argumentsRef": arguments_ref.as_str(),
                }),
            )
            .await?;
            let active_run = self.active_run_handle(run_id).await?;
            remove_live_tool_call(&active_run.live_projection, invocation_id, tool_call_index);
            let started = Instant::now();

            if let Err(rejection) = gate.authorize_and_reserve(
                &prepared.tool_snapshot,
                &prepared.tool_turn,
                tool_invocation,
            ) {
                let budget_message = match &rejection {
                    ToolRequestGateError::InvocationBudgetExhausted { max_calls } => Some(format!(
                        "Agent tool call budget is exhausted for this invocation (max {max_calls})."
                    )),
                    ToolRequestGateError::ToolBudgetExhausted { max_calls, .. } => Some(format!(
                        "Agent profile tool call budget for `{tool_name}` is exhausted (max {max_calls})."
                    )),
                    _ => None,
                };
                if let Some(message) = budget_message {
                    let outcome = recoverable_tool_error(
                        tool_invocation,
                        "agent.tool_budget_exhausted",
                        &message,
                        started.elapsed().as_millis(),
                    );
                    return Ok(outcome);
                }

                let error = if matches!(
                    &rejection,
                    ToolRequestGateError::TurnSnapshotMismatch { .. }
                ) {
                    ApplicationError::InternalError(rejection.to_string())
                } else {
                    ApplicationError::ValidationError(rejection.to_string())
                };
                self.event(
                    run_id,
                    AgentRunEventLevel::Error,
                    "tool_call_failed",
                    json!({
                        "round": round,
                        "invocationId": invocation_id,
                        "callId": tool_invocation.call_id.as_str(),
                        "toolId": tool_invocation.tool_id.as_str(),
                        "snapshotId": snapshot_id,
                        "name": tool_name,
                        "message": error.to_string(),
                    }),
                )
                .await?;
                return Err(error);
            }

            // Charge the budget before rejecting arguments so malformed calls cannot retry for free.
            let args = match tool_invocation.arguments.as_map() {
                Ok(args) => args,
                Err(message) => {
                    return Ok(recoverable_tool_error(
                        tool_invocation,
                        "tool.invalid_arguments",
                        &message,
                        started.elapsed().as_millis(),
                    ));
                }
            };

            let call = tool_invocation;
            if exit_policy == AgentInvocationExitPolicy::RunFinishAllowed {
                self.transition_status(run_id, AgentRunStatus::DispatchingTool)
                    .await?;
            }
            self.event(
                run_id,
                AgentRunEventLevel::Info,
                "tool_call_started",
                json!({
                    "round": round,
                    "invocationId": invocation_id,
                    "callId": call.call_id.as_str(),
                    "toolId": tool_invocation.tool_id.as_str(),
                    "snapshotId": snapshot_id,
                    "name": tool_name,
                }),
            )
            .await?;

            let builtin_name = call.tool_id.is_builtin().then_some(tool_name);
            if builtin_name == Some("chat.search") && args.get("read_matches").and_then(Value::as_bool) == Some(true)
                && !prepared.tool_turn.tools().iter().any(|id| id.is_builtin() && id.native_name() == "chat.read_messages") {
                return Ok(recoverable_tool_error(call, "chat.read_permission_required", "read_matches requires an available chat.read_messages tool", started.elapsed().as_millis()));
            }
            let finishing_task = builtin_name == Some("workspace.write_file") && args.get("finish_task").and_then(Value::as_bool) == Some(true);
            let task_return_available = exit_policy == AgentInvocationExitPolicy::TaskReturnRequired
                && prepared.tool_turn.tools().iter().any(|id| id.is_builtin() && id.native_name() == TASK_RETURN);
            let finishing_commit = builtin_name == Some("workspace.commit") && args.get("finish").and_then(Value::as_bool) == Some(true);
            let finish_available = exit_policy == AgentInvocationExitPolicy::RunFinishAllowed
                && prepared.tool_turn.tools().iter().any(|id| id.is_builtin() && id.native_name() == WORKSPACE_FINISH);
            let state_mutation = matches!(builtin_name, Some("workspace.write_file" | "workspace.apply_patch"));
            if state_mutation && args.get("finalize").is_some_and(|value| !value.is_boolean()) {
                return Ok(recoverable_tool_error(call, "tool.invalid_arguments", "finalize must be a boolean", started.elapsed().as_millis()));
            }
            let finalizing_state = state_mutation && args.get("finalize").and_then(Value::as_bool) == Some(true);
            let body_sha = if finalizing_state {
                let can_commit = prepared.tool_turn.tools().iter().any(|id| id.is_builtin() && id.native_name() == "workspace.commit");
                if !finish_available || !can_commit || !is_last_call || tool_call_index != 0 || finishing_task || !args.get("path").and_then(Value::as_str).is_some_and(|path| path.starts_with("persist/")) {
                    return Ok(recoverable_tool_error(call, "agent.finalize_not_ready", "finalize requires a sole persist mutation, commit/finish permissions, no finish_task, and previously delivered reviewed body", started.elapsed().as_millis()));
                }
                match self.reviewed_body_sha(prepared, session).await {
                    Ok(sha) if commit_ledger.matches_delivered_body(&profile.output.message_body_path, &sha) => Some(sha),
                    Ok(_) => return Ok(recoverable_tool_error(call, "agent.finalize_not_ready", "finalize requires the unchanged complete body from a confirmed replace commit; append fragments, unpublished changes, and legacy commits without a recorded SHA require explicit publication first", started.elapsed().as_millis())),
                    Err(ApplicationError::ValidationError(message)) | Err(ApplicationError::NotFound(message)) => return Ok(recoverable_tool_error(call, "agent.finalize_not_ready", &message, started.elapsed().as_millis())),
                    Err(error) => return Err(error),
                }
            } else { None };
            let mut capabilities = Vec::new();
            if builtin_name == Some("chat.search") && args.get("read_matches").and_then(Value::as_bool) == Some(true) {
                capabilities.push(ToolId::builtin("chat.read_messages")?);
            }
            if finishing_task && task_return_available && is_last_call {
                capabilities.push(ToolId::builtin(TASK_RETURN)?);
            }
            if finishing_commit && finish_available && is_last_call {
                capabilities.push(ToolId::builtin(WORKSPACE_FINISH)?);
            }
            if finalizing_state {
                capabilities.extend([ToolId::builtin("workspace.commit")?, ToolId::builtin(WORKSPACE_FINISH)?]);
            }
            if let Err(error) = gate.check_capabilities(&prepared.tool_snapshot, &prepared.tool_turn, &capabilities) {
                if builtin_name == Some("chat.search") {
                    let mut search_args = args.clone();
                    search_args.insert("read_matches".into(), Value::Bool(false));
                    let repository = InvocationWorkspaceRepository::new(self.workspace_repository.as_ref(), profile);
                    started_tool = true;
                    let mut outcome = self.tool_dispatcher.dispatch_with_model_workspace_repository(
                        run_id, call, &search_args, session, profile, &repository,
                    ).await?;
                    if !outcome.result.is_error {
                        let message = error.to_string();
                        outcome.result.content = format!("Search succeeded, but reading its matches failed: {message}\n\n{}", outcome.result.content);
                        outcome.result.structured["readError"] = json!({"code":error.code(),"message":message});
                        outcome.result.is_error = true;
                        outcome.result.error_code = Some(error.code().into());
                    }
                    return Ok(outcome);
                }
                return Ok(recoverable_tool_error(call, error.code(), &error.to_string(), started.elapsed().as_millis()));
            }
            if builtin_name == Some("chat.search") {
                gate.reserve_capabilities(&prepared.tool_snapshot, &prepared.tool_turn, &capabilities)
                    .map_err(|error| ApplicationError::ValidationError(error.to_string()))?;
            }
            started_tool = true;
            let dispatch_result = if finishing_task && !task_return_available {
                Ok(recoverable_tool_error(call, "agent.task_return_denied", "finish_task requires a return-mode child and task.return permission", started.elapsed().as_millis()))
            } else if finishing_commit && !finish_available {
                Ok(recoverable_tool_error(call, "agent.finish_unavailable", "commit with finish=true requires an invocation allowed to finish and an available workspace.finish tool", started.elapsed().as_millis()))
            } else if !is_last_call && (finishing_commit || finishing_task || builtin_name.is_some_and(is_completion_tool)) {
                Ok(recoverable_tool_error(
                    call,
                    "agent.tool_after_finish",
                    &format!(
                        "{} must be the final tool call in a model turn; complete the other work first, then call it again.",
                        call.tool_id.native_name()
                    ),
                    started.elapsed().as_millis(),
                ))
            } else if builtin_name == Some(AGENT_LIST) {
                self.dispatch_agent_list_tool(call, args, profile).await
            } else if builtin_name == Some(AGENT_DELEGATE) {
                let wait = args.get("wait").and_then(Value::as_bool) == Some(true);
                if wait && !prepared.tool_turn.tools().iter().any(|id| id.is_builtin() && id.native_name() == AGENT_AWAIT) {
                    Ok(recoverable_tool_error(call, "agent.await_unavailable", "wait=true requires agent.await permission", started.elapsed().as_millis()))
                } else {
                    let delegated = Box::pin(self.dispatch_agent_delegate_tool(prepared, call, args, session, gate)).await?;
                    if wait && !delegated.result.is_error {
                        gate.reserve_capabilities(&prepared.tool_snapshot, &prepared.tool_turn, &[ToolId::builtin(AGENT_AWAIT)?])
                            .map_err(|error| ApplicationError::ValidationError(error.to_string()))?;
                        let task_id = delegated.result.structured.get("taskId").and_then(Value::as_str).ok_or_else(|| ApplicationError::InternalError("delegation result has no taskId".into()))?;
                        let await_args = json!({"taskIds":[task_id],"mode":"allCompleted","readArtifacts":args.get("readArtifacts").and_then(Value::as_bool).unwrap_or(false)});
                        let mut outcome = self.dispatch_agent_await_tool(prepared, call, await_args.as_object().expect("object arguments"), commit_ledger.explicit_count(), session, gate, cancel).await?;
                        outcome.result.structured["taskId"] = json!(task_id);
                        if outcome.result.is_error {
                            outcome.result.content = format!("Delegated task `{task_id}` already exists; do not delegate it again. {}", outcome.result.content);
                        }
                        Ok(outcome)
                    } else { Ok(delegated) }
                }
            } else if builtin_name == Some(AGENT_AWAIT) {
                self.dispatch_agent_await_tool(
                    prepared,
                    call,
                    args,
                    commit_ledger.explicit_count(),
                    session,
                    gate,
                    cancel,
                )
                .await
            } else if builtin_name == Some(AGENT_HANDOFF) {
                self.dispatch_agent_handoff_tool(run_id, invocation_id, call, args, profile)
                    .await
            } else if builtin_name == Some(TASK_RETURN) {
                self.dispatch_task_return_tool(
                    run_id,
                    invocation_id,
                    call,
                    args,
                    exit_policy,
                    profile,
                )
                .await
            } else if !call.tool_id.is_builtin() {
                // The MCP service reports sent-but-unconfirmed calls explicitly.
                started_tool = false;
                let outcome = self.call_mcp_tool(call, args, cancel).await?;
                started_tool = true;
                match outcome {
                    McpCallOutcome::KnownResponse(response) => Ok(AgentToolDispatchOutcome {
                        result: mcp_known_response_result(call, response),
                        effect: AgentToolEffect::None,
                        elapsed_ms: started.elapsed().as_millis(),
                    }),
                    McpCallOutcome::NotSent(issue) => Ok(recoverable_tool_error(
                        call,
                        issue.code.as_str(),
                        issue.message.as_str(),
                        started.elapsed().as_millis(),
                    )),
                    McpCallOutcome::OutcomeUnknown(issue) => {
                        let message = format!(
                            "mcp.call_outcome_unknown: {} The MCP tool may have executed; this call will not be retried.",
                            issue.message
                        );
                        let outcome = recoverable_tool_error(
                            call,
                            "mcp.call_outcome_unknown",
                            &message,
                            started.elapsed().as_millis(),
                        );
                        let _ = self
                            .record_tool_outcome(run_id, invocation_id, round, snapshot_id, &outcome)
                            .await?;
                        return Err(if *cancel.borrow() {
                            ApplicationError::Cancelled(message)
                        } else {
                            ApplicationError::ValidationError(message)
                        });
                    }
                }
            } else {
                let workspace_repository =
                    InvocationWorkspaceRepository::new(self.workspace_repository.as_ref(), profile);
                self.tool_dispatcher
                    .dispatch_with_model_workspace_repository(
                        run_id,
                        call,
                        args,
                        session,
                        profile,
                        &workspace_repository,
                    )
                    .await
            };

            match dispatch_result {
                Ok(mut outcome) => {
                    ensure_tool_result_identity(tool_invocation, &outcome.result)?;
                    if finishing_task && !outcome.result.is_error {
                        gate.reserve_capabilities(&prepared.tool_snapshot, &prepared.tool_turn, &[ToolId::builtin(TASK_RETURN)?])
                            .map_err(|error| ApplicationError::ValidationError(error.to_string()))?;
                        outcome = self.return_written_artifact(prepared, call, outcome).await?;
                    }
                    let mutation_receipt = if !outcome.result.is_error && body_sha.is_some() { Some(outcome.result.structured.clone()) } else { None };
                    if let Some(sha) = body_sha.as_deref() && !outcome.result.is_error {
                        outcome = self.finalize_successful_mutation(prepared, outcome, sha).await?;
                    }
                    if let AgentToolEffect::ChatCommitRequested { path, mode, reason, finish } = outcome.effect.clone() {
                        if finalizing_state {
                            gate.reserve_capabilities(&prepared.tool_snapshot, &prepared.tool_turn, &[ToolId::builtin("workspace.commit")?])
                                .map_err(|error| ApplicationError::ValidationError(error.to_string()))?;
                        }
                        outcome = self.perform_explicit_host_chat_commit(
                            run_id, call, path, body_sha.as_deref(), mode, reason, outcome.elapsed_ms, round,
                            invocation_id, commit_ledger, cancel,
                        ).await?;
                        if let Some(receipt) = mutation_receipt {
                            outcome.result.structured["savedMutation"] = receipt;
                            if outcome.result.is_error {
                                outcome.result.content = format!("State operation succeeded but publication did not. Do not repeat the state operation; fix publication and retry commit. {}", outcome.result.content);
                            }
                        }
                        if finish && !outcome.result.is_error {
                            gate.reserve_capabilities(&prepared.tool_snapshot, &prepared.tool_turn, &[ToolId::builtin(WORKSPACE_FINISH)?])
                                .map_err(|error| ApplicationError::ValidationError(error.to_string()))?;
                            outcome.effect = AgentToolEffect::Finish;
                            outcome.result.content = "Chat commit confirmed. Finalizing the run and publishing pending workspace changes without another model request.".to_string();
                            outcome.result.structured["finishing"] = json!(true);
                        }
                    }
                    let outcome = match outcome.effect.clone() {
                        AgentToolEffect::Finish => {
                            if exit_policy == AgentInvocationExitPolicy::TaskReturnRequired {
                                recoverable_tool_error(
                                    tool_invocation,
                                    "agent.child_finish_denied",
                                    "Return-mode child Agent invocations must complete with task.return, not workspace.finish.",
                                    outcome.elapsed_ms,
                                )
                            } else if !commit_ledger.has_explicit_commit()
                                && self.run_repository.load_run(run_id).await?.presentation
                                    == AgentRunPresentation::Foreground
                            {
                                recoverable_tool_error(
                                    tool_invocation,
                                    "agent.foreground_commit_required",
                                    "Foreground Agent runs must call workspace.commit successfully before workspace.finish.",
                                    outcome.elapsed_ms,
                                )
                            } else {
                                if self.has_pending_child_tasks(run_id, invocation_id).await? {
                                    self.active_run_handle(run_id)
                                        .await?
                                        .scheduler
                                        .cancel_unfinished_for_parent(invocation_id)
                                        .await?;
                                }
                                outcome
                            }
                        }
                        _ => outcome,
                    };
                    Ok(outcome)
                }
                Err(error) => {
                    self.event(
                        run_id,
                        AgentRunEventLevel::Error,
                        "tool_call_failed",
                        json!({
                            "round": round,
                            "invocationId": invocation_id,
                            "callId": call.call_id.as_str(),
                            "toolId": tool_invocation.tool_id.as_str(),
                            "snapshotId": snapshot_id,
                            "name": tool_name,
                            "message": error.to_string(),
                        }),
                    )
                    .await?;
                    Err(error)
                }
            }
        }
        .await;
        result.map_err(|error| ToolCallFailure {
            error,
            started: started_tool,
        })
    }

    pub(super) async fn record_tool_outcome_for_model(
        &self,
        prepared: &PreparedInvocation,
        round: usize,
        outcome: &mut AgentToolDispatchOutcome,
    ) -> Result<(), ApplicationError> {
        let run_id = prepared.invocation.run_id.as_str();
        let invocation_id = prepared.invocation.id.as_str();
        let snapshot_id = prepared.tool_snapshot.id().as_str();
        let profile = &prepared.profile;
        let result_path = self
            .record_tool_outcome(run_id, invocation_id, round, snapshot_id, outcome)
            .await?;
        if !outcome.result.tool_id.is_builtin() {
            let readable_path = WorkspacePath::parse(format!(
                "tool-results/{invocation_id}/round-{round:03}-{}.txt",
                tool_call_audit_file_stem(&outcome.result.call_id)
            ))?;
            let mut projected = outcome.result.clone();
            if let Some(readable) = project_mcp_result_for_model(
                &mut projected,
                &result_path,
                &readable_path,
                &prepared.tool_snapshot,
                profile.tools.mcp_result_inline_char_limit,
            )? {
                self.workspace_repository
                    .write_text_guarded(
                        run_id,
                        &readable_path,
                        &readable,
                        WorkspaceWriteGuard::MustNotExist,
                    )
                    .await?;
                self.event(
                    run_id,
                    AgentRunEventLevel::Debug,
                    "tool_result_readable_view_stored",
                    json!({
                        "invocationId": invocation_id,
                        "round": round,
                        "callId": outcome.result.call_id.as_str(),
                        "toolId": outcome.result.tool_id.as_str(),
                        "path": readable_path.as_str(),
                        "auditPath": result_path.as_str(),
                    }),
                )
                .await?;
            }
            outcome.result = projected;
        }
        Ok(())
    }

    async fn call_mcp_tool(
        &self,
        call: &ToolInvocation,
        args: &Map<String, Value>,
        cancel: &mut super::AgentCancelReceiver,
    ) -> Result<McpCallOutcome, ApplicationError> {
        let cancellation = CancellationToken::new();
        if *cancel.borrow() {
            cancellation.cancel();
        }
        let watcher = if cancellation.is_cancelled() {
            None
        } else {
            let cancellation = cancellation.clone();
            let mut receiver = cancel.clone();
            Some(tokio::spawn(async move {
                if receiver.changed().await.is_ok() && *receiver.borrow() {
                    cancellation.cancel();
                }
            }))
        };
        let outcome = self
            .mcp_service
            .call_permitted_tool(&call.tool_id, Value::Object(args.clone()), cancellation)
            .await;
        if let Some(watcher) = watcher {
            watcher.abort();
        }

        outcome
    }

    async fn record_tool_outcome(
        &self,
        run_id: &str,
        invocation_id: &str,
        round: usize,
        snapshot_id: &str,
        outcome: &AgentToolDispatchOutcome,
    ) -> Result<WorkspacePath, ApplicationError> {
        let path = self
            .store_tool_result(run_id, invocation_id, round, &outcome.result)
            .await?;
        let error_message = outcome.result.is_error.then(|| {
            if outcome.result.tool_id.is_builtin() {
                outcome.result.content.clone()
            } else {
                format!("MCP tool returned an error; full result: {}", path.as_str())
            }
        });
        self.event(
            run_id,
            if outcome.result.is_error {
                AgentRunEventLevel::Warn
            } else {
                AgentRunEventLevel::Info
            },
            if outcome.result.is_error {
                "tool_call_failed"
            } else {
                "tool_call_completed"
            },
            json!({
                "round": round,
                "invocationId": invocation_id,
                "callId": outcome.result.call_id.as_str(),
                "toolId": outcome.result.tool_id.as_str(),
                "snapshotId": snapshot_id,
                "name": outcome.result.tool_id.native_name(),
                "isError": outcome.result.is_error,
                "errorCode": outcome.result.error_code.as_deref(),
                "message": error_message,
                "elapsedMs": outcome.elapsed_ms,
                "resourceRefs": &outcome.result.resource_refs,
            }),
        )
        .await?;
        Ok(path)
    }

    async fn store_tool_result(
        &self,
        run_id: &str,
        invocation_id: &str,
        round: usize,
        result: &AgentToolResult,
    ) -> Result<WorkspacePath, ApplicationError> {
        let path = WorkspacePath::parse(format!(
            "tool-results/{invocation_id}/round-{round:03}-{}.json",
            tool_call_audit_file_stem(&result.call_id)
        ))?;
        let text = serde_json::to_string_pretty(result).map_err(|error| {
            ApplicationError::ValidationError(format!(
                "agent.tool_result_serialize_failed: {error}"
            ))
        })?;
        self.workspace_repository
            .write_text_guarded(run_id, &path, &text, WorkspaceWriteGuard::MustNotExist)
            .await?;
        self.event(
            run_id,
            AgentRunEventLevel::Debug,
            "tool_result_stored",
            json!({
                "invocationId": invocation_id,
                "round": round,
                "callId": result.call_id.as_str(),
                "toolId": result.tool_id.as_str(),
                "path": path.as_str(),
            }),
        )
        .await?;
        Ok(path)
    }

    async fn store_tool_arguments(
        &self,
        run_id: &str,
        invocation_id: &str,
        round: usize,
        call: &ToolInvocation,
    ) -> Result<WorkspacePath, ApplicationError> {
        let path = WorkspacePath::parse(format!(
            "tool-args/{invocation_id}/round-{round:03}-{}.json",
            tool_call_audit_file_stem(&call.call_id)
        ))?;
        let text = serde_json::to_string_pretty(&call.arguments).map_err(|error| {
            ApplicationError::ValidationError(format!(
                "agent.tool_arguments_serialize_failed: {error}"
            ))
        })?;
        self.workspace_repository
            .write_text_guarded(run_id, &path, &text, WorkspaceWriteGuard::MustNotExist)
            .await?;
        Ok(path)
    }
}

fn project_mcp_result_for_model(
    result: &mut AgentToolResult,
    audit_path: &WorkspacePath,
    readable_path: &WorkspacePath,
    snapshot: &InvocationToolSnapshot,
    inline_char_limit: usize,
) -> Result<Option<String>, ApplicationError> {
    result.content = mcp_model_content(result);
    let char_count = TextMetrics::from_text(&result.content).chars;
    if char_count <= inline_char_limit {
        return Ok(None);
    }

    let readable = line_addressable_content(&result.content);
    externalize_mcp_result(
        result,
        audit_path,
        readable_path,
        snapshot,
        char_count,
        inline_char_limit,
    )?;
    Ok(Some(readable))
}

fn mcp_model_content(result: &AgentToolResult) -> String {
    let structured_content = result
        .structured
        .get("structuredContent")
        .filter(|value| !value.is_null());
    let mut sections = Vec::new();
    let text = result.content.trim();
    if !text.is_empty()
        && !structured_content.is_some_and(|value| text_is_serialized_value(text, value))
    {
        sections.push(text.to_string());
    }

    if let Some(value) = structured_content {
        sections.push(markdown_value_section("Details", value));
    }

    let notes = result
        .structured
        .get("diagnostics")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|diagnostic| {
            diagnostic.get("code").and_then(Value::as_str) != Some("mcp.call_metadata_unsupported")
        })
        .filter_map(|diagnostic| diagnostic.get("message").and_then(Value::as_str))
        .map(|message| format!("- {}", message.trim()))
        .collect::<Vec<_>>();
    if !notes.is_empty() {
        sections.push(format!("## Notes\n\n{}", notes.join("\n")));
    }

    if let Some(server_error) = result.structured.get("serverError")
        && let Some(data) = server_error.get("data").filter(|value| !value.is_null())
    {
        sections.push(markdown_value_section("Error details", data));
    }

    if sections.is_empty() {
        "The MCP tool completed without content.".to_string()
    } else {
        sections.join("\n\n")
    }
}

fn markdown_value_section(title: &str, value: &Value) -> String {
    format!("## {title}\n\n{}", render_markdown_value(value, 0))
}

fn text_is_serialized_value(text: &str, value: &Value) -> bool {
    serde_json::from_str::<Value>(text).is_ok_and(|parsed| parsed == *value)
}

fn externalize_mcp_result(
    result: &mut AgentToolResult,
    audit_path: &WorkspacePath,
    readable_path: &WorkspacePath,
    snapshot: &InvocationToolSnapshot,
    char_count: usize,
    inline_char_limit: usize,
) -> Result<(), ApplicationError> {
    let preview = result
        .content
        .chars()
        .take(MCP_RESULT_CONTENT_CHUNK_CHARS)
        .collect::<String>();
    let read_tool = ToolId::builtin("workspace.read_file")?;
    let read_alias = snapshot
        .binding(&read_tool)
        .map(|binding| binding.model_alias());
    let search_tool = ToolId::builtin("workspace.search_files")?;
    let search_alias = snapshot
        .binding(&search_tool)
        .map(|binding| binding.model_alias());
    let mut instructions = format!(
        "This MCP result is too large to include here ({char_count} characters; inline limit {inline_char_limit}). The complete readable result is available at `{}`.",
        readable_path.as_str(),
    );
    if let Some(alias) = read_alias {
        instructions.push_str(&format!(
            " Use {alias} with path `{}` to read it. If it returns a preview, continue from the reported nextStartLine using start_line. Long source lines are wrapped so the complete result remains reachable.",
            readable_path.as_str()
        ));
    } else {
        instructions.push_str(
            " This Agent does not have a text-reading tool, so the available prefix is included below and the full path remains available for the user.",
        );
    }
    if let Some(alias) = search_alias {
        instructions.push_str(&format!(
            " Use {alias} with path `{}` to locate specific text before reading exact ranges.",
            readable_path.as_str()
        ));
    }
    if !preview.is_empty() {
        instructions.push_str(&format!(
            "\n\n## Prefix preview\n\nThe following is at most {MCP_RESULT_CONTENT_CHUNK_CHARS} Unicode characters and is not the complete result.\n\n{preview}"
        ));
    }
    result.content = instructions;
    result.structured = json!({
        "externalized": true,
        "path": readable_path.as_str(),
        "auditPath": audit_path.as_str(),
        "charCount": char_count,
        "charLimit": inline_char_limit,
    });
    for path in [readable_path, audit_path] {
        if !result
            .resource_refs
            .iter()
            .any(|reference| reference == path.as_str())
        {
            result.resource_refs.push(path.as_str().to_string());
        }
    }
    Ok(())
}

fn line_addressable_content(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    let mut line_chars = 0;
    for character in text.chars() {
        if character == '\n' {
            output.push(character);
            line_chars = 0;
            continue;
        }
        if line_chars == MCP_RESULT_CONTENT_CHUNK_CHARS {
            output.push('\n');
            line_chars = 0;
        }
        output.push(character);
        line_chars += 1;
    }
    output
}

fn mcp_known_response_result(call: &ToolInvocation, response: McpKnownResponse) -> AgentToolResult {
    match response {
        McpKnownResponse::ToolResult(result) => {
            let content = result
                .text
                .iter()
                .map(|block| block.text.as_str())
                .collect::<Vec<_>>()
                .join("\n\n");
            let diagnostics = result
                .diagnostics
                .iter()
                .map(|diagnostic| {
                    json!({
                        "code": diagnostic.code,
                        "message": diagnostic.message,
                        "contentIndex": diagnostic.content_index,
                    })
                })
                .collect::<Vec<_>>();
            AgentToolResult {
                call_id: call.call_id.clone(),
                tool_id: call.tool_id.clone(),
                content,
                structured: json!({
                    "structuredContent": result.structured_content,
                    "diagnostics": diagnostics,
                }),
                is_error: result.is_error,
                error_code: result.is_error.then(|| "mcp.tool_error".to_string()),
                resource_refs: Vec::new(),
            }
        }
        McpKnownResponse::ServerError(error) => AgentToolResult {
            call_id: call.call_id.clone(),
            tool_id: call.tool_id.clone(),
            content: error.message.clone(),
            structured: json!({
                "serverError": {
                    "code": error.code,
                    "message": error.message,
                    "data": error.data,
                }
            }),
            is_error: true,
            error_code: Some("mcp.server_error".to_string()),
            resource_refs: Vec::new(),
        },
        McpKnownResponse::Unsupported(response) => AgentToolResult {
            call_id: call.call_id.clone(),
            tool_id: call.tool_id.clone(),
            content: response.message.clone(),
            structured: json!({
                "unsupportedResponse": {
                    "type": response.response_type,
                    "message": response.message,
                }
            }),
            is_error: true,
            error_code: Some("mcp.unsupported_response".to_string()),
            resource_refs: Vec::new(),
        },
    }
}

fn tool_call_audit_file_stem(call_id: &str) -> String {
    let digest = Sha256::digest(call_id.as_bytes());
    format!(
        "call_{}",
        hex_lower(&digest[..TOOL_CALL_AUDIT_DIGEST_BYTES])
    )
}

fn is_completion_tool(tool_name: &str) -> bool {
    matches!(tool_name, WORKSPACE_FINISH | AGENT_HANDOFF | TASK_RETURN)
}

pub(super) fn recoverable_tool_error(
    call: &ToolInvocation,
    code: &str,
    message: &str,
    elapsed_ms: u128,
) -> AgentToolDispatchOutcome {
    AgentToolDispatchOutcome {
        result: AgentToolResult {
            call_id: call.call_id.clone(),
            tool_id: call.tool_id.clone(),
            content: message.to_string(),
            structured: json!({
                "error": {
                    "code": code,
                    "message": message,
                }
            }),
            is_error: true,
            error_code: Some(code.to_string()),
            resource_refs: Vec::new(),
        },
        effect: AgentToolEffect::None,
        elapsed_ms,
    }
}

fn ensure_tool_result_identity(
    invocation: &ToolInvocation,
    result: &AgentToolResult,
) -> Result<(), ApplicationError> {
    if result.call_id == invocation.call_id && result.tool_id == invocation.tool_id {
        return Ok(());
    }
    Err(ApplicationError::InternalError(format!(
        "tool.result_identity_mismatch: invocation `{}` / `{}` produced result `{}` / `{}`",
        invocation.call_id, invocation.tool_id, result.call_id, result.tool_id
    )))
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};
    use tt_domain::models::agent::AgentToolResult;
    use tt_domain::models::agent::WorkspacePath;
    use tt_domain::models::tool::{
        InvocationToolSnapshot, ToolArguments, ToolBinding, ToolDescriptor, ToolId, ToolInvocation,
        ToolProviderId, ToolSnapshotId,
    };

    use super::{
        MCP_RESULT_CONTENT_CHUNK_CHARS, ensure_tool_result_identity, mcp_model_content,
        project_mcp_result_for_model,
    };

    #[test]
    fn tool_result_identity_must_match_its_invocation() {
        let invocation = ToolInvocation {
            call_id: "call_1".to_string(),
            tool_id: ToolId::builtin("workspace.finish").unwrap(),
            arguments: ToolArguments::empty(),
            provider_metadata: Value::Null,
        };
        let result = AgentToolResult {
            call_id: invocation.call_id.clone(),
            tool_id: ToolId::builtin("workspace.commit").unwrap(),
            content: String::new(),
            structured: Value::Null,
            is_error: false,
            error_code: None,
            resource_refs: Vec::new(),
        };

        let error = ensure_tool_result_identity(&invocation, &result).unwrap_err();
        assert!(error.to_string().contains("tool.result_identity_mismatch"));
    }

    #[test]
    fn mcp_model_content_keeps_actionable_structured_data() {
        let result = AgentToolResult {
            call_id: "call_mcp".to_string(),
            tool_id: ToolId::new(
                &ToolProviderId::parse("mcp/550e8400-e29b-41d4-a716-446655440000").unwrap(),
                "search",
            )
            .unwrap(),
            content: "Created issue.".to_string(),
            structured: json!({
                "structuredContent": { "issueId": 42 },
                "diagnostics": [{
                    "code": "mcp.call_content_unsupported",
                    "message": "Image content is not supported",
                    "contentIndex": 1,
                }, {
                    "code": "mcp.call_metadata_unsupported",
                    "message": "Result metadata is not supported",
                    "contentIndex": null,
                }],
            }),
            is_error: false,
            error_code: None,
            resource_refs: Vec::new(),
        };

        let content = mcp_model_content(&result);
        assert!(content.contains("Created issue."));
        assert!(content.contains("## Details"));
        assert!(content.contains("- **issueId**: 42"));
        assert!(content.contains("## Notes"));
        assert!(content.contains("- Image content is not supported"));
        assert!(!content.contains("\"issueId\""));
        assert!(!content.contains("mcp.call_content_unsupported"));
        assert!(!content.contains("Result metadata"));
    }

    #[test]
    fn mcp_model_content_deduplicates_serialized_structured_data() {
        let result = AgentToolResult {
            call_id: "call_mcp".to_string(),
            tool_id: ToolId::new(
                &ToolProviderId::parse("mcp/550e8400-e29b-41d4-a716-446655440000").unwrap(),
                "lookup",
            )
            .unwrap(),
            content: r#"{"issueId":42}"#.to_string(),
            structured: json!({
                "structuredContent": { "issueId": 42 },
                "diagnostics": [],
            }),
            is_error: false,
            error_code: None,
            resource_refs: Vec::new(),
        };

        assert_eq!(
            mcp_model_content(&result),
            "## Details\n\n- **issueId**: 42"
        );
    }

    #[test]
    fn externalized_mcp_result_points_to_readable_full_artifact() {
        let read_id = ToolId::builtin("workspace.read_file").unwrap();
        let snapshot = InvocationToolSnapshot::try_new(
            ToolSnapshotId::parse("snapshot").unwrap(),
            vec![
                ToolBinding::new(
                    ToolDescriptor {
                        id: read_id,
                        title: None,
                        description: None,
                        input_schema: json!({ "type": "object" }),
                        output_schema: None,
                        annotations: json!({}),
                    },
                    "workspace_read_file",
                    None,
                )
                .unwrap(),
            ],
            2,
        )
        .unwrap();
        let mut result = AgentToolResult {
            call_id: "call_mcp".to_string(),
            tool_id: ToolId::new(
                &ToolProviderId::parse("mcp/550e8400-e29b-41d4-a716-446655440000").unwrap(),
                "search",
            )
            .unwrap(),
            content: "full content".to_string(),
            structured: json!({ "full": true }),
            is_error: false,
            error_code: None,
            resource_refs: Vec::new(),
        };
        let audit_path = WorkspacePath::parse("tool-results/call_deadbeef.json").unwrap();
        let readable_path = WorkspacePath::parse("tool-results/call_deadbeef.txt").unwrap();
        let inline_char_limit = 50_000;

        assert!(
            project_mcp_result_for_model(
                &mut result,
                &audit_path,
                &readable_path,
                &snapshot,
                inline_char_limit,
            )
            .unwrap()
            .is_none()
        );
        assert_eq!(result.content, "full content");

        result.content = format!(
            "{}outside-preview{}",
            "界".repeat(MCP_RESULT_CONTENT_CHUNK_CHARS),
            "x".repeat(inline_char_limit)
        );
        let readable = project_mcp_result_for_model(
            &mut result,
            &audit_path,
            &readable_path,
            &snapshot,
            inline_char_limit,
        )
        .unwrap()
        .expect("large MCP result should produce a readable view");

        assert!(result.content.contains("workspace_read_file"));
        assert!(result.content.contains(readable_path.as_str()));
        assert!(!result.content.contains(audit_path.as_str()));
        assert!(result.content.contains("## Prefix preview"));
        assert_eq!(
            result.content.matches('界').count(),
            MCP_RESULT_CONTENT_CHUNK_CHARS
        );
        assert!(!result.content.contains("outside-preview"));
        assert!(readable.contains("outside-preview"));
        assert!(
            readable
                .lines()
                .all(|line| { line.chars().count() <= MCP_RESULT_CONTENT_CHUNK_CHARS })
        );
        assert_eq!(result.structured["externalized"], true);
        assert_eq!(result.structured["path"], readable_path.as_str());
        assert_eq!(result.structured["auditPath"], audit_path.as_str());
        assert!(result.structured["charCount"].as_u64().unwrap() > 50_000);
        assert_eq!(result.structured["charLimit"], inline_char_limit);
        assert_eq!(
            result.resource_refs,
            vec![
                readable_path.as_str().to_string(),
                audit_path.as_str().to_string()
            ]
        );
    }
}
