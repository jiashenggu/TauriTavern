use std::collections::HashSet;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use super::rendering::{DelegatedResultContinuationHint, render_await_content};
use super::task_status::task_is_terminal;
use super::tool_error::tool_error_outcome;
use crate::errors::ApplicationError;
use crate::services::agent_runtime_service::{
    AgentCancelReceiver, AgentRuntimeService, PreparedInvocation,
};
use crate::services::agent_tools::{AgentToolDispatchOutcome, AgentToolEffect, AgentToolSession};
use crate::services::tool_request_gate::ToolRequestGate;
use tt_domain::models::agent::{
    AgentRunEventLevel, AgentTaskRecord, AgentTaskStatus, AgentToolResult,
};
use tt_domain::models::tool::ToolInvocation;

const DEFAULT_AGENT_AWAIT_TIMEOUT_MS: u64 = 120_000;
const MAX_AGENT_AWAIT_TIMEOUT_MS: u64 = 300_000;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AgentAwaitArgs {
    #[serde(default)]
    task_ids: Option<Vec<String>>,
    #[serde(default)]
    mode: Option<AgentAwaitMode>,
    #[serde(default)]
    timeout_ms: Option<u64>,
    #[serde(default)]
    read_artifacts: bool,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
enum AgentAwaitMode {
    NextCompleted,
    AllCompleted,
    StatusOnly,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::services::agent_runtime_service) struct AwaitTaskView {
    task_id: String,
    agent_id: String,
    status: AgentTaskStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    confidence: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    artifacts: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    findings: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    warnings: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    suggested_next_actions: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    questions_for_caller: Option<Value>,
}

impl AgentRuntimeService {
    #[expect(
        clippy::too_many_arguments,
        reason = "await shares the invocation read session and budget for artifact delivery"
    )]
    pub(in crate::services::agent_runtime_service) async fn dispatch_agent_await_tool(
        &self,
        prepared: &PreparedInvocation,
        call: &ToolInvocation,
        args: &Map<String, Value>,
        committed_count: usize,
        session: &mut AgentToolSession,
        gate: &mut ToolRequestGate,
        cancel: &mut AgentCancelReceiver,
    ) -> Result<AgentToolDispatchOutcome, ApplicationError> {
        let run_id = prepared.invocation.run_id.as_str();
        let invocation_id = prepared.invocation.id.as_str();
        let profile = &prepared.profile;
        let parent_tools = &prepared.request.tools;
        let started = Instant::now();
        let args = match serde_json::from_value::<AgentAwaitArgs>(Value::Object(args.clone())) {
            Ok(args) => args,
            Err(error) => {
                return Ok(tool_error_outcome(
                    call,
                    "tool.invalid_arguments",
                    &format!("invalid agent.await arguments: {error}"),
                    started.elapsed().as_millis(),
                ));
            }
        };
        let mode = args.mode.unwrap_or(AgentAwaitMode::NextCompleted);
        let timeout_ms = args.timeout_ms.unwrap_or(DEFAULT_AGENT_AWAIT_TIMEOUT_MS);
        if timeout_ms > MAX_AGENT_AWAIT_TIMEOUT_MS {
            return Ok(tool_error_outcome(
                call,
                "tool.invalid_arguments",
                &format!("timeoutMs must be <= {MAX_AGENT_AWAIT_TIMEOUT_MS}"),
                started.elapsed().as_millis(),
            ));
        }
        let selected_ids = match normalize_task_ids(args.task_ids) {
            Ok(ids) => ids,
            Err(message) => {
                return Ok(tool_error_outcome(
                    call,
                    "tool.invalid_arguments",
                    &message,
                    started.elapsed().as_millis(),
                ));
            }
        };
        self.event(
            run_id,
            AgentRunEventLevel::Info,
            "agent_await_started",
            json!({
                "parentInvocationId": invocation_id,
                "mode": mode,
                "taskIds": selected_ids,
                "timeoutMs": timeout_ms,
            }),
        )
        .await?;

        let deadline = Instant::now() + Duration::from_millis(timeout_ms);
        let scheduler = self.active_run_handle(run_id).await?.scheduler.clone();
        let mut task_events = scheduler.subscribe();
        let (tasks, timed_out) = loop {
            if *cancel.borrow() {
                return Ok(tool_error_outcome(
                    call,
                    "agent.await_interrupted",
                    "Waiting ended because this run stopped. Query the delegated tasks again after resuming.",
                    started.elapsed().as_millis(),
                ));
            }
            let tasks = match self
                .selected_child_tasks(run_id, invocation_id, selected_ids.as_ref())
                .await
            {
                Ok(tasks) => tasks,
                Err(ApplicationError::ValidationError(message))
                    if message.contains("agent.await_task_not_found") =>
                {
                    return Ok(tool_error_outcome(
                        call,
                        "agent.await_task_not_found",
                        &message,
                        started.elapsed().as_millis(),
                    ));
                }
                Err(error) => return Err(error),
            };
            if tasks.is_empty() {
                return Ok(tool_error_outcome(
                    call,
                    "agent.no_child_tasks",
                    "No delegated tasks are selected.",
                    started.elapsed().as_millis(),
                ));
            }
            if await_condition_met(&tasks, mode) {
                break (tasks, false);
            }
            if mode == AgentAwaitMode::StatusOnly || Instant::now() >= deadline {
                break (tasks, mode != AgentAwaitMode::StatusOnly);
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            tokio::select! {
                changed = task_events.changed() => {
                    changed.map_err(|_| ApplicationError::InternalError(format!(
                        "agent.task_scheduler_closed: active run `{run_id}` task scheduler closed while awaiting delegated tasks"
                    )))?;
                }
                _ = tokio::time::sleep(remaining) => {}
                changed = cancel.changed() => {
                    let _ = changed;
                }
            }
        };

        let views = self.await_task_views(run_id, &tasks).await?;
        let mut structured = json!({
            "mode": mode,
            "timedOut": timed_out,
            "tasks": views,
        });
        let continuation_hint = DelegatedResultContinuationHint::from_parent_tools(
            parent_tools,
            profile.run.presentation,
            committed_count,
        );
        let mut content = render_await_content(&structured, Some(&continuation_hint));
        let mut artifact_error = None;
        if args.read_artifacts {
            let reads = async {
                let files = artifact_files(&views)?;
                self.read_context_files(prepared, session, gate, &files, false)
                    .await
            }
            .await;
            match reads {
                Ok(reads) => {
                    if !reads.is_empty() {
                        content.push_str(&format!(
                            "\n\n{}",
                            super::super::workspace_context::context_text(&reads)
                        ));
                    }
                    structured["artifactReads"] = json!(
                        reads
                            .iter()
                            .map(|read| &read.structured)
                            .collect::<Vec<_>>()
                    );
                }
                Err(error) => {
                    let message = error.to_string();
                    structured["readError"] =
                        json!({"code":"agent.artifact_read_failed","message":message});
                    content.push_str(&format!("\n\nArtifact reading failed: {message}. Task results above are retained; query or read these existing tasks rather than delegating them again."));
                    artifact_error = Some("agent.artifact_read_failed".to_string());
                }
            }
        }
        self.event(
            run_id,
            if timed_out || artifact_error.is_some() {
                AgentRunEventLevel::Warn
            } else {
                AgentRunEventLevel::Info
            },
            "agent_await_completed",
            json!({
                "parentInvocationId": invocation_id,
                "mode": mode,
                "timedOut": timed_out,
                "taskCount": structured["tasks"].as_array().map(Vec::len).unwrap_or(0),
                "artifactReadFailed": artifact_error.is_some(),
            }),
        )
        .await?;

        Ok(AgentToolDispatchOutcome {
            result: AgentToolResult {
                call_id: call.call_id.clone(),
                tool_id: call.tool_id.clone(),
                content,
                structured,
                is_error: artifact_error.is_some(),
                error_code: artifact_error,
                resource_refs: Vec::new(),
            },
            effect: AgentToolEffect::None,
            elapsed_ms: started.elapsed().as_millis(),
        })
    }

    async fn selected_child_tasks(
        &self,
        run_id: &str,
        invocation_id: &str,
        selected_ids: Option<&Vec<String>>,
    ) -> Result<Vec<AgentTaskRecord>, ApplicationError> {
        let tasks = self.invocation_repository.list_tasks(run_id).await?;
        let mut owned = tasks
            .into_iter()
            .filter(|task| task.parent_invocation_id == invocation_id)
            .collect::<Vec<_>>();
        let Some(selected_ids) = selected_ids else {
            return Ok(owned);
        };
        let mut selected = Vec::with_capacity(selected_ids.len());
        for selected_id in selected_ids {
            let Some(index) = owned.iter().position(|task| task.id == *selected_id) else {
                return Err(ApplicationError::ValidationError(format!(
                    "agent.await_task_not_found: task `{selected_id}` does not belong to invocation `{invocation_id}`"
                )));
            };
            selected.push(owned.swap_remove(index));
        }
        selected.sort_by(|left, right| {
            left.created_at
                .cmp(&right.created_at)
                .then_with(|| left.id.cmp(&right.id))
        });
        Ok(selected)
    }

    pub(in crate::services::agent_runtime_service) async fn await_task_views(
        &self,
        run_id: &str,
        tasks: &[AgentTaskRecord],
    ) -> Result<Vec<AwaitTaskView>, ApplicationError> {
        let mut views = Vec::with_capacity(tasks.len());
        for task in tasks {
            let result = self.read_task_result(run_id, task).await?;
            let output_field = |key| {
                result
                    .as_ref()
                    .and_then(|result| result.output.get(key))
                    .filter(|value| !value.is_null())
                    .cloned()
            };
            views.push(AwaitTaskView {
                task_id: task.id.clone(),
                agent_id: task.target_profile_id.clone(),
                status: task.status,
                error: task.error.clone(),
                summary: result.as_ref().map(|result| result.summary.clone()),
                confidence: output_field("confidence"),
                artifacts: output_field("artifacts"),
                findings: output_field("findings"),
                warnings: output_field("warnings"),
                suggested_next_actions: output_field("suggestedNextActions"),
                questions_for_caller: output_field("questionsForCaller"),
            });
        }
        Ok(views)
    }

    pub(in crate::services::agent_runtime_service) async fn completed_child_results_message(
        &self,
        prepared: &PreparedInvocation,
        seen_task_ids: &mut HashSet<String>,
        committed_count: usize,
    ) -> Result<Option<String>, ApplicationError> {
        let run_id = prepared.invocation.run_id.as_str();
        let invocation_id = prepared.invocation.id.as_str();
        let profile = &prepared.profile;
        let parent_tools = &prepared.request.tools;
        let tasks = self
            .invocation_repository
            .list_tasks(run_id)
            .await?
            .into_iter()
            .filter(|task| task.parent_invocation_id == invocation_id)
            .filter(|task| task_is_terminal(task.status))
            .filter(|task| !seen_task_ids.contains(&task.id))
            .collect::<Vec<_>>();
        if tasks.is_empty() {
            return Ok(None);
        }
        let views = self.await_task_views(run_id, &tasks).await?;
        for task in &tasks {
            seen_task_ids.insert(task.id.clone());
        }
        let structured = json!({
            "mode": "backgroundResults",
            "timedOut": false,
            "tasks": views,
        });
        let continuation_hint = DelegatedResultContinuationHint::from_parent_tools(
            parent_tools,
            profile.run.presentation,
            committed_count,
        );
        Ok(Some(format!(
            "Delegated task results are now available. Review them before deciding your next action.\n\n{}",
            render_await_content(&structured, Some(&continuation_hint))
        )))
    }
}

fn artifact_files(
    views: &[AwaitTaskView],
) -> Result<Vec<(String, Option<String>)>, ApplicationError> {
    let mut files = Vec::new();
    for view in views
        .iter()
        .filter(|view| view.status == AgentTaskStatus::Completed)
    {
        let Some(artifacts) = &view.artifacts else {
            continue;
        };
        let artifacts = artifacts.as_array().ok_or_else(|| {
            ApplicationError::ValidationError(format!(
                "agent.artifact_invalid: task {} artifacts must be an array",
                view.task_id
            ))
        })?;
        for artifact in artifacts {
            let (path, sha) =
                super::task_return_tool::artifact_reference(artifact).map_err(|error| {
                    ApplicationError::ValidationError(format!(
                        "agent.artifact_invalid: task {}: {}",
                        view.task_id, error.message
                    ))
                })?;
            files.push((path.as_str().to_owned(), sha));
        }
    }
    Ok(files)
}

fn normalize_task_ids(task_ids: Option<Vec<String>>) -> Result<Option<Vec<String>>, String> {
    let Some(task_ids) = task_ids else {
        return Ok(None);
    };
    let mut normalized = Vec::with_capacity(task_ids.len());
    for task_id in task_ids {
        let task_id = task_id.trim();
        if task_id.is_empty() {
            return Err("taskIds cannot contain empty values".to_string());
        }
        if normalized.iter().any(|existing| existing == task_id) {
            return Err(format!("duplicate task id `{task_id}`"));
        }
        normalized.push(task_id.to_string());
    }
    Ok(Some(normalized))
}

fn await_condition_met(tasks: &[AgentTaskRecord], mode: AgentAwaitMode) -> bool {
    match mode {
        AgentAwaitMode::StatusOnly => true,
        AgentAwaitMode::NextCompleted => tasks.iter().any(|task| task_is_terminal(task.status)),
        AgentAwaitMode::AllCompleted => tasks.iter().all(|task| task_is_terminal(task.status)),
    }
}
