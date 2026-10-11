use super::*;
use tt_domain::models::tool::ToolId;
use tt_domain::models::upstream_failure::{UPSTREAM_NETWORK_TIMEOUT, UpstreamFailure};

#[tokio::test]
async fn agent_runtime_combined_reads_and_waits_share_underlying_tool_budgets() {
    use tt_domain::models::chat::ChatMessage;
    let root = temp_root("combined-read-budgets");
    let fixture = agent_runtime_fixture_with_responses(
        &root,
        vec![
            model_tool_response(vec![
                model_tool_call(
                    "seed",
                    "workspace_write_file",
                    json!({"path":"scratch/facts.json","content":"{}"}),
                ),
                model_tool_call(
                    "denied_initial_read",
                    "workspace_read_file",
                    json!({"path":"scratch/facts.json"}),
                ),
                model_tool_call(
                    "denied_workspace_ref",
                    "agent_delegate",
                    json!({"agentId":"scene-critic","task":{"objective":"must not start"},"contextRefs":[{"kind":"workspace","path":"scratch/facts.json"}]}),
                ),
                model_tool_call(
                    "search_once",
                    "chat_search",
                    json!({"query":"lantern","read_matches":true}),
                ),
                model_tool_call(
                    "denied_chat_ref",
                    "agent_delegate",
                    json!({"agentId":"scene-critic","task":{"objective":"must not start"},"contextRefs":[{"kind":"chat","index":0}]}),
                ),
                model_tool_call(
                    "denied_search_read",
                    "chat_search",
                    json!({"query":"lantern","read_matches":true}),
                ),
                model_tool_call(
                    "world_once",
                    "worldinfo_read_activated",
                    json!({"entries":[{"ref":"worldinfo:test#0"}]}),
                ),
                model_tool_call(
                    "denied_world_ref",
                    "agent_delegate",
                    json!({"agentId":"scene-critic","task":{"objective":"must not start"},"contextRefs":[{"kind":"worldInfo","ref":"worldinfo:test#0"}]}),
                ),
                model_tool_call("await_once", "agent_await", json!({"mode":"statusOnly"})),
                model_tool_call(
                    "denied_wait",
                    "agent_delegate",
                    json!({"agentId":"scene-critic","task":{"objective":"must not start"},"wait":true}),
                ),
            ]),
            model_tool_response(vec![model_tool_call(
                "finish",
                "workspace_finish",
                json!({}),
            )]),
        ],
    );
    super::delegation::configure_return_mode_profiles(&fixture).await;
    let mut definition = fixture
        .profile_service
        .load_profile("default-writer")
        .await
        .unwrap()
        .unwrap();
    definition.context.initial_workspace_files = vec!["persist/missing.json".into()];
    for name in [
        "workspace.read_file",
        "chat.read_messages",
        "worldinfo.read_activated",
        "agent.await",
    ] {
        definition
            .tools
            .max_calls_per_tool
            .insert(format!("builtin:{name}"), 1);
    }
    fixture
        .profile_service
        .save_profile(definition, fixture.service.tool_catalog())
        .await
        .unwrap();
    let profile = fixture
        .profile_service
        .resolve_profile(AgentProfileResolveInput {
            profile_id: Some("default-writer"),
            tool_catalog: fixture.service.tool_catalog(),
        })
        .await
        .unwrap();
    let mut run = contract_run(
        "combined-read-budgets",
        AgentRunPresentation::Background,
        &profile,
    );
    run.chat_ref = AgentChatRef::Character {
        character_id: "Alice".into(),
        file_name: "budgets.jsonl".into(),
    };
    run.input_message_count = Some(1);
    let mut chat = Chat::new("User", "Alice");
    chat.file_name = Some("budgets.jsonl".into());
    chat.add_message(ChatMessage::user("User", "blue lantern"));
    fixture.chat_repository.save(&chat).await.unwrap();
    fixture.agent_repository.create_run(&run).await.unwrap();
    let request = chat_request("check read budgets");
    let prompt = json!({"chatCompletionPayload":request.payload,"worldInfoActivation":{"entries":[{"world":"test","uid":0,"content":"reference facts"}]}});
    let (_sender, mut cancel) = watch::channel(false);
    fixture
        .service
        .execute_agent_loop_run_inner(&run.id, prompt, request, profile, &mut cancel)
        .await
        .unwrap();
    let requests = fixture.model_gateway.requests().await;
    let results = requests
        .last()
        .unwrap()
        .messages
        .iter()
        .flat_map(|message| &message.parts)
        .filter_map(|part| {
            if let AgentModelContentPart::ToolResult { result } = part {
                Some((result.call_id.as_str(), result))
            } else {
                None
            }
        })
        .collect::<std::collections::HashMap<_, _>>();
    for id in [
        "denied_initial_read",
        "denied_workspace_ref",
        "denied_chat_ref",
        "denied_search_read",
        "denied_world_ref",
        "denied_wait",
    ] {
        assert_eq!(
            results[id].error_code.as_deref(),
            Some("agent.tool_budget_exhausted"),
            "{id}"
        );
    }
    assert_eq!(
        results["search_once"].structured["messageReads"]["messages"][0]["text"],
        "blue lantern"
    );
    assert_eq!(
        results["denied_search_read"].structured["hits"],
        results["search_once"].structured["hits"]
    );
    assert!(
        results["denied_search_read"]
            .structured
            .get("messageReads")
            .is_none()
    );
    assert!(!results["world_once"].is_error);
    assert!(
        fixture
            .agent_repository
            .list_tasks(&run.id)
            .await
            .unwrap()
            .is_empty()
    );
    fs::remove_dir_all(root).await.unwrap();
}

#[tokio::test]
async fn agent_runtime_artifact_reads_honor_exhausted_budget_and_preserve_task_results() {
    let root = temp_root("artifact-read-budget");
    let fixture = agent_runtime_fixture_with_responses(
        &root,
        vec![
            model_tool_response(vec![
                model_tool_call(
                    "seed",
                    "workspace_write_file",
                    json!({"path":"scratch/seed.json","content":"{}"}),
                ),
                model_tool_call(
                    "allowed_read",
                    "workspace_read_file",
                    json!({"path":"scratch/seed.json"}),
                ),
                model_tool_call(
                    "delegate",
                    "agent_delegate",
                    json!({"agentId":"scene-critic","task":{"objective":"Return the artifact"},"wait":true,"readArtifacts":true}),
                ),
            ]),
            model_tool_response(vec![model_tool_call(
                "artifact",
                "workspace_write_file",
                json!({"path":"scratch/artifact.json","content":"{\"detail\":\"new artifact data\"}","finish_task":true}),
            )]),
            model_tool_response(vec![
                model_tool_call(
                    "denied_read",
                    "workspace_read_file",
                    json!({"path":"scratch/artifact.json"}),
                ),
                model_tool_call("recover", "agent_await", json!({"mode":"statusOnly"})),
            ]),
            model_tool_response(vec![model_tool_call(
                "finish",
                "workspace_finish",
                json!({}),
            )]),
        ],
    );
    super::delegation::configure_return_mode_profiles(&fixture).await;
    let mut definition = fixture
        .profile_service
        .load_profile("default-writer")
        .await
        .unwrap()
        .unwrap();
    definition.tools.max_rounds = 3;
    definition
        .tools
        .max_calls_per_tool
        .insert("builtin:workspace.read_file".into(), 1);
    fixture
        .profile_service
        .save_profile(definition, fixture.service.tool_catalog())
        .await
        .unwrap();
    let profile = resolve_contract_profile(&fixture).await;
    let handle = start_contract_agent_run(
        &fixture,
        &profile,
        AgentRunPresentation::Background,
        "artifact-read-budget",
        Some(false),
    )
    .await;
    let run = wait_for_terminal_agent_run(&fixture.agent_repository, &handle.run_id).await;
    assert_eq!(run.status, AgentRunStatus::Completed);
    let requests = fixture.model_gateway.requests().await;
    let results = requests
        .last()
        .unwrap()
        .messages
        .iter()
        .flat_map(|message| &message.parts)
        .filter_map(|part| {
            if let AgentModelContentPart::ToolResult { result } = part {
                Some((result.call_id.as_str(), result))
            } else {
                None
            }
        })
        .collect::<std::collections::HashMap<_, _>>();
    let delegated = results["delegate"];
    assert!(delegated.is_error);
    assert!(delegated.content.contains("agent.tool_budget_exhausted"));
    assert!(!delegated.content.contains("new artifact data"));
    assert_eq!(delegated.structured["tasks"][0]["status"], "completed");
    assert_eq!(
        delegated.structured["tasks"][0]["taskId"],
        results["recover"].structured["tasks"][0]["taskId"]
    );
    assert_eq!(
        results["denied_read"].error_code.as_deref(),
        Some("agent.tool_budget_exhausted")
    );
    assert!(
        super::resume::wait_for_checkpoint(&fixture, &handle.run_id)
            .await
            .blocked_reason
            .is_none()
    );
    fs::remove_dir_all(root).await.unwrap();
}

#[tokio::test]
async fn agent_runtime_finalize_reserves_commit_budget_before_mutating_state() {
    let root = temp_root("finalize-budget");
    let fixture = agent_runtime_fixture_with_responses(
        &root,
        vec![
            model_tool_response(vec![
                model_tool_call(
                    "body",
                    "workspace_write_file",
                    json!({"path":"output/main.md","content":"reviewed body"}),
                ),
                model_tool_call(
                    "state",
                    "workspace_write_file",
                    json!({"path":"persist/state.json","content":"{\"n\":0}"}),
                ),
                model_tool_call("commit_once", "workspace_commit", json!({})),
            ]),
            model_tool_response(vec![model_tool_call(
                "denied_finalize",
                "workspace_apply_patch",
                json!({"path":"persist/state.json","json_ops":[{"op":"replace","path":"/n","value":99}],"finalize":true}),
            )]),
            model_tool_response(vec![model_tool_call(
                "finish",
                "workspace_finish",
                json!({}),
            )]),
        ],
    );
    let mut definition = fixture
        .profile_service
        .load_profile("default-writer")
        .await
        .unwrap()
        .unwrap();
    definition.tools.max_rounds = 3;
    definition
        .tools
        .max_calls_per_tool
        .insert("builtin:workspace.commit".into(), 1);
    fixture
        .profile_service
        .save_profile(definition, fixture.service.tool_catalog())
        .await
        .unwrap();
    let profile = resolve_contract_profile(&fixture).await;
    let handle = start_contract_agent_run(
        &fixture,
        &profile,
        AgentRunPresentation::Foreground,
        "finalize-budget",
        Some(false),
    )
    .await;
    let (run, resolver) = tokio::join!(
        wait_for_terminal_agent_run(&fixture.agent_repository, &handle.run_id),
        resolve_chat_commits_and_persistent_state_update(
            fixture.service.clone(),
            fixture.agent_repository.clone(),
            handle.run_id.clone(),
            "0",
            &[]
        )
    );
    resolver.unwrap();
    assert_eq!(run.status, AgentRunStatus::Completed);
    assert_eq!(
        read_workspace_json(
            &fixture.agent_repository,
            &handle.run_id,
            "persist/state.json"
        )
        .await,
        json!({"n":0})
    );
    let events = read_agent_events(&fixture.agent_repository, &handle.run_id).await;
    assert!(
        events
            .iter()
            .any(|event| event.event_type == "tool_call_failed"
                && event.payload["callId"] == "denied_finalize"
                && event.payload["errorCode"] == "agent.tool_budget_exhausted")
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| event.event_type == "chat_commit_requested")
            .count(),
        1
    );
    fs::remove_dir_all(root).await.unwrap();
}

#[tokio::test]
async fn agent_runtime_combined_completion_cannot_bypass_finish_or_total_budget() {
    let root = temp_root("combined-finish-budget");
    let fixture = agent_runtime_fixture_with_responses(
        &root,
        vec![
            model_tool_response(vec![model_tool_call(
                "premature_finish",
                "workspace_finish",
                json!({}),
            )]),
            model_tool_response(vec![model_tool_call(
                "denied_finish",
                "workspace_commit",
                json!({"finish":true}),
            )]),
        ],
    );
    let mut definition = fixture
        .profile_service
        .load_profile("default-writer")
        .await
        .unwrap()
        .unwrap();
    definition.tools.max_rounds = 2;
    definition
        .tools
        .max_calls_per_tool
        .insert("builtin:workspace.finish".into(), 1);
    fixture
        .profile_service
        .save_profile(definition, fixture.service.tool_catalog())
        .await
        .unwrap();
    let profile = resolve_contract_profile(&fixture).await;
    let handle = start_contract_agent_run(
        &fixture,
        &profile,
        AgentRunPresentation::Foreground,
        "combined-finish-budget",
        Some(false),
    )
    .await;
    let run = wait_for_terminal_agent_run(&fixture.agent_repository, &handle.run_id).await;
    assert_eq!(run.status, AgentRunStatus::Failed);
    let events = read_agent_events(&fixture.agent_repository, &handle.run_id).await;
    assert!(
        events
            .iter()
            .any(|event| event.event_type == "tool_call_failed"
                && event.payload["callId"] == "denied_finish"
                && event.payload["errorCode"] == "agent.tool_budget_exhausted")
    );
    assert!(
        !events
            .iter()
            .any(|event| event.event_type == "chat_commit_requested")
    );
    fs::remove_dir_all(root).await.unwrap();

    let root = temp_root("combined-return-budget");
    let fixture = agent_runtime_fixture_with_responses(
        &root,
        vec![
            model_tool_response(vec![model_tool_call(
                "delegate",
                "agent_delegate",
                json!({"agentId":"scene-critic","task":{"objective":"Return artifact"},"wait":true}),
            )]),
            model_tool_response(vec![model_tool_call(
                "denied_return",
                "workspace_write_file",
                json!({"path":"scratch/rejected.json","content":"{}","finish_task":true}),
            )]),
            model_tool_response(vec![model_tool_call(
                "finish",
                "workspace_finish",
                json!({}),
            )]),
        ],
    );
    let profile = super::delegation::configure_return_mode_profiles(&fixture).await;
    let mut child = fixture
        .profile_service
        .load_profile("scene-critic")
        .await
        .unwrap()
        .unwrap();
    child.tools.max_calls_per_run = 1;
    fixture
        .profile_service
        .save_profile(child, fixture.service.tool_catalog())
        .await
        .unwrap();
    let handle = start_contract_agent_run(
        &fixture,
        &profile,
        AgentRunPresentation::Background,
        "combined-return-budget",
        Some(false),
    )
    .await;
    let run = wait_for_terminal_agent_run(&fixture.agent_repository, &handle.run_id).await;
    assert_eq!(run.status, AgentRunStatus::Completed);
    assert!(
        fixture
            .agent_repository
            .read_text(
                &handle.run_id,
                &WorkspacePath::parse("scratch/rejected.json").unwrap()
            )
            .await
            .is_err()
    );
    let events = read_agent_events(&fixture.agent_repository, &handle.run_id).await;
    assert!(
        events
            .iter()
            .any(|event| event.event_type == "tool_call_failed"
                && event.payload["callId"] == "denied_return"
                && event.payload["errorCode"] == "agent.tool_budget_exhausted")
    );
    fs::remove_dir_all(root).await.unwrap();
}

#[tokio::test]
async fn agent_runtime_invalid_artifact_sha_can_be_corrected_before_returning() {
    let root = temp_root("artifact-sha-recovery");
    let fixture = agent_runtime_fixture_with_responses(
        &root,
        vec![
            model_tool_response(vec![model_tool_call(
                "delegate",
                "agent_delegate",
                json!({"agentId":"scene-critic","task":{"objective":"Return artifact"},"wait":true,"readArtifacts":true}),
            )]),
            model_tool_response(vec![
                model_tool_call(
                    "draft",
                    "workspace_write_file",
                    json!({"path":"scratch/draft.json","content":"{\"ready\":true}"}),
                ),
                model_tool_call(
                    "invalid_return",
                    "task_return",
                    json!({"summary":"Artifact ready","artifacts":[{"path":"scratch/draft.json","sha256":null}]}),
                ),
            ]),
            model_tool_response(vec![model_tool_call(
                "corrected_return",
                "task_return",
                json!({"summary":"Artifact ready","artifacts":[{"path":"scratch/draft.json"}]}),
            )]),
            model_tool_response(vec![model_tool_call(
                "finish",
                "workspace_finish",
                json!({}),
            )]),
        ],
    );
    let profile = super::delegation::configure_return_mode_profiles(&fixture).await;
    let mut child = fixture
        .profile_service
        .load_profile("scene-critic")
        .await
        .unwrap()
        .unwrap();
    child.tools.max_rounds = 2;
    fixture
        .profile_service
        .save_profile(child, fixture.service.tool_catalog())
        .await
        .unwrap();
    let handle = start_contract_agent_run(
        &fixture,
        &profile,
        AgentRunPresentation::Background,
        "artifact-sha-recovery",
        Some(false),
    )
    .await;
    let run = wait_for_terminal_agent_run(&fixture.agent_repository, &handle.run_id).await;
    assert_eq!(run.status, AgentRunStatus::Completed);
    let events = read_agent_events(&fixture.agent_repository, &handle.run_id).await;
    assert!(
        events
            .iter()
            .any(|event| event.event_type == "tool_call_failed"
                && event.payload["callId"] == "invalid_return"
                && event.payload["errorCode"] == "tool.invalid_arguments")
    );
    let requests = fixture.model_gateway.requests().await;
    assert_eq!(requests.len(), 4);
    assert!(requests.last().unwrap().messages.iter().flat_map(|message| &message.parts).any(|part| matches!(part, AgentModelContentPart::ToolResult { result } if result.call_id == "delegate" && !result.is_error && result.structured["artifactReads"][0]["fullRead"] == true)));
    assert!(
        super::resume::wait_for_checkpoint(&fixture, &handle.run_id)
            .await
            .blocked_reason
            .is_none()
    );
    fs::remove_dir_all(root).await.unwrap();
}

#[tokio::test]
async fn zhaowei_writer_returns_readable_artifact_without_publishing_or_mutating_memory() {
    let root = temp_root("zhaowei-writer-artifact");
    let body = "场景原稿：信使已到。\n下一步由玩家决定。";
    let final_body = body.replace("原稿", "成稿");
    let fixture = agent_runtime_fixture_with_responses(
        &root,
        vec![
            model_tool_response(vec![
                model_tool_call(
                    "parent_memory",
                    "workspace_write_file",
                    json!({ "path": "persist/state.json", "content": "{\"owner\":\"controller\"}" }),
                ),
                model_tool_call(
                    "parent_context",
                    "workspace_write_file",
                    json!({ "path": "scratch/context.json", "content": "{\"scene\":\"harbor\"}" }),
                ),
                model_tool_call(
                    "delegate_writer",
                    "agent_delegate",
                    json!({ "agentId": "zhaowei-scene-writer", "task": { "objective": "Write scratch/draft.md and return its artifact reference." } }),
                ),
                model_tool_call(
                    "await_writer",
                    "agent_await",
                    json!({ "mode": "allCompleted", "timeoutMs": 5_000 }),
                ),
            ]),
            model_tool_response(vec![
                model_tool_call(
                    "writer_private_read",
                    "workspace_read_file",
                    json!({ "path": "persist/state.json" }),
                ),
                model_tool_call(
                    "writer_private_write",
                    "workspace_write_file",
                    json!({ "path": "persist/state.json", "content": "{\"owner\":\"writer\"}" }),
                ),
                model_tool_call(
                    "writer_output_write",
                    "workspace_write_file",
                    json!({ "path": "output/main.md", "content": "unreviewed" }),
                ),
                model_tool_call(
                    "writer_private_copy",
                    "workspace_write_file",
                    json!({ "path": "scratch/private.json", "source_path": "persist/state.json" }),
                ),
                model_tool_call(
                    "writer_read_context",
                    "workspace_read_file",
                    json!({ "path": "scratch/context.json" }),
                ),
            ]),
            model_tool_response(vec![
                model_tool_call(
                    "writer_draft",
                    "workspace_write_file",
                    json!({ "path": "scratch/draft.md", "content": body }),
                ),
                model_tool_call(
                    "writer_context",
                    "workspace_write_file",
                    json!({ "path": "scratch/context.json", "content": "{\"scene\":\"updated\"}" }),
                ),
            ]),
            model_tool_response(vec![model_tool_call(
                "writer_return",
                "task_return",
                json!({ "summary": "Draft saved.", "artifacts": [{ "path": "scratch/draft.md", "kind": "markdown", "role": "sceneDraft" }] }),
            )]),
            model_tool_response(vec![
                model_tool_call(
                    "parent_partial_draft",
                    "workspace_read_file",
                    json!({ "path": "scratch/draft.md", "start_line": 1, "line_count": 1 }),
                ),
                model_tool_call(
                    "parent_partial_copy",
                    "workspace_write_file",
                    json!({ "path": "output/partial.json", "source_path": "scratch/draft.md" }),
                ),
                model_tool_call(
                    "parent_stale_copy",
                    "workspace_write_file",
                    json!({ "path": "output/stale.json", "source_path": "scratch/context.json" }),
                ),
            ]),
            model_tool_response(vec![model_tool_call(
                "parent_read_draft",
                "workspace_read_file",
                json!({ "path": "scratch/draft.md", "start_line": 2, "line_count": 1 }),
            )]),
            model_tool_response(vec![
                model_tool_call(
                    "parent_invalid_copy",
                    "workspace_write_file",
                    json!({ "path": "output/rejected.json", "source_path": "scratch/draft.md", "edits": [
                        { "old_string": "原稿", "new_string": "损坏" },
                        { "old_string": "not-present", "new_string": "invalid" }
                    ] }),
                ),
                model_tool_call(
                    "parent_final",
                    "workspace_write_file",
                    json!({ "path": "output/main.md", "source_path": "scratch/draft.md", "edits": [{ "old_string": "原稿", "new_string": "成稿" }] }),
                ),
                model_tool_call(
                    "parent_auxiliary",
                    "workspace_write_file",
                    json!({ "path": "scratch/receipt.json", "content": "{}" }),
                ),
            ]),
            model_tool_response(vec![model_tool_call(
                "parent_invalid_state",
                "workspace_apply_patch",
                json!({
                    "path": "persist/state.json", "old_string": "\"controller\"", "new_string": "invalid", "validate_json": true
                }),
            )]),
            model_tool_response(vec![
                model_tool_call(
                    "parent_state",
                    "workspace_apply_patch",
                    json!({
                        "path": "persist/state.json", "old_string": "\"controller\"", "new_string": "\"controller\",\"phase\":\"saved\"", "validate_json": true
                    }),
                ),
                model_tool_call("parent_commit", "workspace_commit", json!({})),
                model_tool_call("parent_finish", "workspace_finish", json!({})),
            ]),
        ],
    );
    let controller = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../docs/presets/zhaowei-agent.profile.json"
    )))
    .unwrap();
    let mut writer: tt_domain::models::agent::profile::AgentProfileDefinition =
        serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../docs/presets/zhaowei-scene-writer.profile.json"
        )))
        .unwrap();
    writer.model = serde_json::from_value(json!({ "mode": "currentPromptSnapshot" })).unwrap();
    for definition in [controller, writer] {
        fixture
            .profile_service
            .save_profile(definition, fixture.service.tool_catalog())
            .await
            .unwrap();
    }
    let profile = fixture
        .profile_service
        .resolve_profile(AgentProfileResolveInput {
            profile_id: Some("zhaowei-agent"),
            tool_catalog: fixture.service.tool_catalog(),
        })
        .await
        .unwrap();
    let handle = start_contract_agent_run(
        &fixture,
        &profile,
        AgentRunPresentation::Foreground,
        "zhaowei-writer-artifact",
        Some(true),
    )
    .await;
    let (run, resolver) = tokio::join!(
        wait_for_terminal_agent_run(&fixture.agent_repository, &handle.run_id),
        resolve_chat_commits_and_persistent_state_update(
            fixture.service.clone(),
            fixture.agent_repository.clone(),
            handle.run_id.clone(),
            "0",
            &[],
        )
    );
    resolver.unwrap();
    assert_eq!(run.status, AgentRunStatus::Completed);
    let events = read_agent_events(&fixture.agent_repository, &handle.run_id).await;
    for call_id in [
        "writer_private_read",
        "writer_private_write",
        "writer_output_write",
        "writer_private_copy",
    ] {
        assert!(events.iter().any(|event| {
            event.event_type == "tool_call_failed" && event.payload["callId"] == call_id
        }));
    }
    assert!(
        events
            .iter()
            .filter(|event| event.event_type == "chat_commit_requested")
            .all(|event| event.payload["path"] == "output/main.md")
    );
    let memory = read_workspace_json(
        &fixture.agent_repository,
        &handle.run_id,
        "persist/state.json",
    )
    .await;
    assert_eq!(memory, json!({ "owner": "controller", "phase": "saved" }));
    let preview = events
        .iter()
        .find(|event| {
            event.event_type == "chat_commit_completed" && event.payload["callId"] == "parent_final"
        })
        .expect("reviewed copied body shown before memory work");
    assert_eq!(preview.payload["isExplicit"], false);
    let state_attempt = events
        .iter()
        .find(|event| {
            event.event_type == "tool_call_requested"
                && event.payload["callId"] == "parent_invalid_state"
        })
        .unwrap();
    assert!(preview.seq < state_attempt.seq);
    assert_eq!(
        events
            .iter()
            .filter(|event| event.event_type == "chat_commit_completed")
            .count(),
        2
    );
    for (id, code) in [
        ("parent_partial_copy", "workspace.copy_requires_read"),
        ("parent_stale_copy", "workspace.copy_stale_source"),
        ("parent_invalid_state", "workspace.invalid_json"),
        (
            "parent_invalid_copy",
            "workspace.patch_old_string_not_found",
        ),
    ] {
        assert!(
            events
                .iter()
                .any(|event| event.event_type == "tool_call_failed"
                    && event.payload["callId"] == id
                    && event.payload["errorCode"] == code)
        );
    }
    for path in [
        "scratch/private.json",
        "output/partial.json",
        "output/stale.json",
        "output/rejected.json",
    ] {
        assert!(
            fixture
                .agent_repository
                .read_text(&handle.run_id, &WorkspacePath::parse(path).unwrap())
                .await
                .is_err()
        );
    }
    assert_eq!(
        fixture
            .agent_repository
            .read_text(
                &handle.run_id,
                &WorkspacePath::parse("output/main.md").unwrap()
            )
            .await
            .unwrap()
            .text,
        final_body
    );
    assert_eq!(
        fixture
            .agent_repository
            .read_text(
                &handle.run_id,
                &WorkspacePath::parse("scratch/draft.md").unwrap()
            )
            .await
            .unwrap()
            .text,
        body
    );
    let requests = fixture.model_gateway.requests().await;
    let caller = requests.last().unwrap();
    assert!(
        tool_result_structured_values(caller, "agent.await")
            .iter()
            .any(|value| value["tasks"][0]["artifacts"][0]["path"] == "scratch/draft.md")
    );
    let reads = caller
        .messages
        .iter()
        .flat_map(|message| &message.parts)
        .filter_map(|part| match part {
            AgentModelContentPart::ToolResult { result }
                if result.tool_id.native_name() == "workspace.read_file"
                    && result.structured["path"] == "scratch/draft.md" =>
            {
                Some(result)
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert!(
        reads
            .iter()
            .any(|result| result.structured["fullRead"] == true)
    );
    for line in body.lines() {
        assert!(reads.iter().any(|result| result.content.contains(line)));
    }
    let _ = fs::remove_dir_all(root).await;
}

#[tokio::test]
async fn agent_runtime_combined_delivery_preserves_child_permissions_and_reads_artifact() {
    use tt_domain::models::chat::ChatMessage;
    let root = temp_root("agent-combined-delivery");
    let raw = "  原文\n  不裁剪《特殊》字符\n尾部  ";
    let facts = json!({"scene":{"raw":raw,"count":3},"unselected":"do not forward this field"});
    let fixture = agent_runtime_fixture_with_responses(
        &root,
        vec![
            model_tool_response(vec![
                model_tool_call(
                    "facts",
                    "workspace_write_file",
                    json!({"path":"persist/reference.json","content":facts.to_string()}),
                ),
                model_tool_call(
                    "ambiguous_source",
                    "workspace_write_file",
                    json!({"path":"scratch/ambiguous.json","content":r#"{"value":{"$serde_json::private::Number":"123"}}"#}),
                ),
                model_tool_call(
                    "ambiguous_ref",
                    "agent_delegate",
                    json!({"agentId":"zhaowei-scene-writer","task":{"objective":"must not start"},"contextRefs":[{"kind":"workspace","path":"scratch/ambiguous.json","pointer":"/value"}]}),
                ),
                model_tool_call(
                    "bad_ref",
                    "agent_delegate",
                    json!({"agentId":"zhaowei-scene-writer","task":{"objective":"must not start"},"contextRefs":[{"kind":"workspace","path":"input/prompt_snapshot.json"}]}),
                ),
                model_tool_call(
                    "preview_ref",
                    "agent_delegate",
                    json!({"agentId":"zhaowei-scene-writer","task":{"objective":"must not start"},"contextRefs":[{"kind":"chat","index":1}]}),
                ),
                model_tool_call(
                    "absent_ref",
                    "agent_delegate",
                    json!({"agentId":"zhaowei-scene-writer","task":{"objective":"must not start"},"contextRefs":[{"kind":"worldInfo","ref":"worldinfo:missing#0"}]}),
                ),
            ]),
            model_tool_response(vec![model_tool_call(
                "delegate",
                "agent_delegate",
                json!({"agentId":"zhaowei-scene-writer","task":{"objective":"Write scratch/delivered.md"},"wait":true,"readArtifacts":true,"contextRefs":[{"kind":"workspace","path":"persist/reference.json","pointer":"/scene"},{"kind":"workspace","path":"persist/reference.json","pointer":"/scene/count"},{"kind":"workspace","path":"persist/reference.json","pointer":"/scene/raw"},{"kind":"chat","index":0},{"kind":"worldInfo","ref":"worldinfo:test#0"}]}),
            )]),
            model_tool_response(vec![model_tool_call(
                "denied_chat_read",
                "chat_search",
                json!({"query":"长","read_matches":true}),
            )]),
            model_tool_response(vec![model_tool_call(
                "denied",
                "workspace_write_file",
                json!({"path":"persist/denied.json","content":"{}","finish_task":true}),
            )]),
            model_tool_response(vec![model_tool_call(
                "deliver",
                "workspace_write_file",
                json!({"path":"scratch/delivered.md","content":"reviewed artifact","create_only":true,"finish_task":true}),
            )]),
            model_tool_response(vec![model_tool_call(
                "copy",
                "workspace_write_file",
                json!({"path":"output/main.md","source_path":"scratch/delivered.md","create_only":true}),
            )]),
            model_tool_response(vec![model_tool_call(
                "commit",
                "workspace_write_file",
                json!({"path":"persist/final.json","content":"{}","validate_json":true,"finalize":true}),
            )]),
        ],
    );
    let controller: tt_domain::models::agent::profile::AgentProfileDefinition =
        serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../docs/presets/zhaowei-agent.profile.json"
        )))
        .unwrap();
    let mut writer: tt_domain::models::agent::profile::AgentProfileDefinition =
        serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../docs/presets/zhaowei-scene-writer.profile.json"
        )))
        .unwrap();
    writer.model = serde_json::from_value(json!({"mode":"currentPromptSnapshot"})).unwrap();
    writer.tools.allow.push("builtin:chat.search".into());
    for profile in [controller, writer] {
        fixture
            .profile_service
            .save_profile(profile, fixture.service.tool_catalog())
            .await
            .unwrap();
    }
    let profile = fixture
        .profile_service
        .resolve_profile(AgentProfileResolveInput {
            profile_id: Some("zhaowei-agent"),
            tool_catalog: fixture.service.tool_catalog(),
        })
        .await
        .unwrap();
    let mut chat = Chat::new("User", "Alice");
    chat.file_name = Some("combined.jsonl".into());
    chat.add_message(ChatMessage::user("User", raw));
    chat.add_message(ChatMessage::user("User", &"长".repeat(100_000)));
    fixture.chat_repository.save(&chat).await.unwrap();
    let request = chat_request("combined");
    let handle = fixture.service.start_run(AgentStartRunDto {
        chat_ref: AgentChatRef::Character { character_id:"Alice".into(), file_name:"combined.jsonl".into() },
        stable_chat_id:"stable-combined".into(), generation_type:"normal".into(), profile_id:Some(profile.id.as_str().into()), persist_base_state_id:None,
        prompt_snapshot:Some(json!({"contextPolicy":profile.context,"chatCompletionPayload":request.payload,"worldInfoActivation":{"entries":[{"world":"test","uid":0,"content":raw}]}})),
        frozen_run_input_snapshot:None, generation_intent:None, skill_scope_refs:AgentSkillScopeRefsDto::default(),
        options:AgentStartRunOptionsDto { stream:Some(true), presentation:Some(AgentRunPresentation::Foreground), ..Default::default() },
    }).await.unwrap();
    let (run, resolver) = tokio::join!(
        wait_for_terminal_agent_run(&fixture.agent_repository, &handle.run_id),
        resolve_chat_commits_and_persistent_state_update(
            fixture.service.clone(),
            fixture.agent_repository.clone(),
            handle.run_id.clone(),
            "0",
            &[]
        )
    );
    resolver.unwrap();
    assert_eq!(run.status, AgentRunStatus::Completed);
    let requests = fixture.model_gateway.requests().await;
    assert_eq!(requests.len(), 7);
    assert!(requests.iter().flat_map(|request| &request.messages).flat_map(|message| &message.parts).any(|part| matches!(part, AgentModelContentPart::ToolResult{result} if result.call_id=="denied_chat_read" && result.error_code.as_deref()==Some("chat.read_permission_required"))));
    let tasks = fixture
        .agent_repository
        .list_tasks(&handle.run_id)
        .await
        .unwrap();
    assert_eq!(tasks.len(), 1);
    let materials = tasks[0].task["referenceMaterials"].as_array().unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(materials[0]["text"].as_str().unwrap()).unwrap(),
        facts["scene"]
    );
    assert_eq!(materials[1]["text"], "3");
    assert_eq!(
        serde_json::from_str::<String>(materials[2]["text"].as_str().unwrap()).unwrap(),
        raw
    );
    assert_eq!(materials[3]["text"], raw);
    assert_eq!(materials[4]["text"], raw);
    let child_texts = requests
        .iter()
        .flat_map(|request| &request.messages)
        .flat_map(|message| &message.parts)
        .filter_map(|part| match part {
            AgentModelContentPart::Text { text } if text.contains("## Reference Data") => {
                Some(text)
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert!(!child_texts.is_empty());
    assert!(
        child_texts
            .iter()
            .all(|text| text.contains(raw) && !text.contains("do not forward this field"))
    );
    let delivered = requests
        .iter()
        .flat_map(|request| request.messages.iter())
        .flat_map(|message| &message.parts)
        .filter_map(|part| match part {
            AgentModelContentPart::ToolResult { result }
                if result.tool_id.native_name() == "agent.delegate" =>
            {
                Some(result)
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert!(
        delivered
            .iter()
            .any(|result| result.content.contains("reviewed artifact")
                && result.structured["artifactReads"][0]["fullRead"] == true)
    );
    assert!(
        fixture
            .agent_repository
            .read_text(
                &handle.run_id,
                &WorkspacePath::parse("persist/denied.json").unwrap()
            )
            .await
            .is_err()
    );
    let events = read_agent_events(&fixture.agent_repository, &handle.run_id).await;
    assert!(
        events
            .iter()
            .any(|event| event.event_type == "tool_call_failed"
                && event.payload["callId"] == "denied")
    );
    assert!(
        events
            .iter()
            .any(|event| event.event_type == "tool_call_failed"
                && event.payload["callId"] == "ambiguous_ref"
                && event.payload["errorCode"] == "agent.context_reference_invalid")
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| event.event_type == "task_return_completed")
            .count(),
        1
    );
    assert!(
        events
            .iter()
            .filter(|event| event.event_type == "chat_commit_requested")
            .all(|event| event.payload["path"] == "output/main.md")
    );
    fs::remove_dir_all(root).await.unwrap();
}

#[tokio::test]
async fn agent_runtime_initial_context_does_not_bypass_workspace_permissions() {
    let root = temp_root("agent-initial-context-denied");
    let fixture = agent_runtime_fixture(&root);
    let mut profile = resolve_contract_profile(&fixture).await;
    profile.context.initial_workspace_files = vec!["input/prompt_snapshot.json".into()];
    let run = contract_run(
        "run_context_denied",
        AgentRunPresentation::Background,
        &profile,
    );
    fixture.agent_repository.create_run(&run).await.unwrap();
    let request = chat_request("read initial context");
    let (_sender, mut cancel) = watch::channel(false);
    let outcome = execute_agent_loop_with_host_resolver(
        fixture.service.clone(),
        run.id.clone(),
        json!({"chatCompletionPayload":request.payload.clone()}),
        request,
        profile,
        &mut cancel,
        async { Ok(()) },
    )
    .await;
    assert!(outcome.is_err());
    assert!(fixture.model_gateway.requests().await.is_empty());
    fs::remove_dir_all(root).await.unwrap();
}

#[tokio::test]
async fn zhaowei_agent_profile_persists_memory_and_isolates_branch_versions() {
    let root = temp_root("zhaowei-agent");
    let body = "<bbs_start>2018年11月1日 07:24</bbs_start><scene_header>商店</scene_header>交易完成。<bbs_end>2018年11月1日 07:25</bbs_end>";
    let states = [
        json!({ "schemaVersion": 1, "revision": 1, "player": { "balance": 80 }, "recentEvents": [{ "id": "E1", "spent": 20 }] }),
        json!({ "schemaVersion": 1, "revision": 2, "player": { "balance": 60 }, "recentEvents": [{ "id": "E1", "spent": 20 }, { "id": "E2", "spent": 20 }] }),
        json!({ "schemaVersion": 1, "revision": 2, "player": { "balance": 70 }, "recentEvents": [{ "id": "E1", "spent": 20 }, { "id": "E2", "spent": 10 }] }),
    ];
    let mut responses = vec![model_tool_response(vec![
        model_tool_call(
            "call_private_draft",
            "workspace_write_file",
            json!({
                "path": "plan/draft.md", "content": "must not be published",
            }),
        ),
        model_tool_call(
            "call_input_write",
            "workspace_write_file",
            json!({
                "path": "input/state.json", "content": "{}",
            }),
        ),
    ])];
    for (index, state) in states.iter().enumerate() {
        if index > 0 {
            responses.push(model_tool_response(vec![
                model_tool_call("call_failed_json_test", "workspace_apply_patch", json!({ "path":"persist/state.json", "json_ops":[
                    {"op":"replace","path":"/player/balance","value":0},
                    {"op":"test","path":"/revision","value":999}
                ]})),
                model_tool_call("call_invalid_batch", "workspace_apply_patch", json!({ "path": "persist/state.json", "edits": [
                    { "old_string": "\"balance\": 80", "new_string": "\"balance\": 0" },
                    { "old_string": "missing-value", "new_string": "invalid" }
                ] })),
                model_tool_call("call_invalid_json", "workspace_apply_patch", json!({ "path": "persist/state.json", "old_string": "\"revision\": 1", "new_string": "\"revision\":", "validate_json": true })),
                model_tool_call("call_missing_key", "workspace_write_file", json!({ "path": "persist/state.json", "content": "{}", "validate_json": true, "required_keys": ["player"] })),
            ]));
        }
        let baseline_args = if index == 0 {
            let baseline = json!({ "schemaVersion": 1, "revision": 0, "player": { "balance": 100 }, "recentEvents": [] });
            json!({ "path": "output/turn-base.json", "content": baseline.to_string(), "create_only": true })
        } else {
            json!({ "path": "scratch/verified-base.json", "source_path": "baseline/persist/state.json", "create_only": true })
        };
        let update = if index == 0 {
            model_tool_call(
                "call_memory",
                "workspace_write_file",
                json!({ "path": "persist/state.json", "source_path":"output/turn-base.json", "required_keys": ["schemaVersion", "revision", "player", "recentEvents"], "json_ops":[
                    {"op":"replace","path":"/revision","value":1},
                    {"op":"replace","path":"/player/balance","value":state["player"]["balance"]},
                    {"op":"add","path":"/recentEvents/-","value":state["recentEvents"][0]}
                ] }),
            )
        } else if index == 1 {
            model_tool_call(
                "call_memory",
                "workspace_apply_patch",
                json!({"path":"persist/state.json","required_keys":["schemaVersion","revision","player","recentEvents"],"json_ops":[
                    {"op":"test","path":"/player/balance","value":80},
                    {"op":"replace","path":"/player/balance","value":state["player"]["balance"]},
                    {"op":"replace","path":"/revision","value":2},
                    {"op":"add","path":"/recentEvents/-","value":state["recentEvents"][1]}
                ]}),
            )
        } else {
            model_tool_call(
                "call_memory",
                "workspace_apply_patch",
                json!({ "path": "persist/state.json", "validate_json": true, "required_keys": ["schemaVersion", "revision", "player", "recentEvents"], "edits": [
                { "old_string": "\"balance\": 80", "new_string": "\"balance\": 79" },
                { "old_string": "\"balance\": 79", "new_string": format!("\"balance\": {}", state["player"]["balance"]) },
                { "old_string": "\"revision\": 1", "new_string": "\"revision\": 2" },
                { "old_string": format!("\"recentEvents\": {}", serde_json::to_string_pretty(&states[0]["recentEvents"]).unwrap().replace('\n', "\n  ")), "new_string": format!("\"recentEvents\": {}", serde_json::to_string_pretty(&state["recentEvents"]).unwrap().replace('\n', "\n  ")) }
            ] }),
            )
        };
        responses.extend([
            model_tool_response(vec![model_tool_call(
                "call_baseline",
                "workspace_write_file",
                baseline_args,
            )]),
            model_tool_response(vec![model_tool_call(
                "call_body",
                "workspace_write_file",
                json!({
                    "path": "output/main.md", "content": body,
                }),
            )]),
            model_tool_response(vec![update]),
            model_tool_response(vec![model_tool_call(
                "call_replace_baseline", "workspace_write_file",
                json!({ "path": if index == 0 { "output/turn-base.json" } else { "baseline/persist/state.json" }, "source_path": "persist/state.json", "create_only": true }),
            )]),
            model_tool_response(vec![model_tool_call(
                "call_publish",
                "workspace_commit",
                json!({ "finish": true }),
            )]),
        ]);
    }
    let fixture = agent_runtime_fixture_with_responses(&root, responses);
    let definition = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../docs/presets/zhaowei-agent.profile.json"
    )))
    .expect("parse shipped Profile");
    let registry = BuiltinAgentToolRegistry::all();
    fixture
        .profile_service
        .save_profile(definition, registry.catalog())
        .await
        .unwrap();
    let mut profile = fixture
        .profile_service
        .resolve_profile(AgentProfileResolveInput {
            profile_id: Some("zhaowei-agent"),
            tool_catalog: registry.catalog(),
        })
        .await
        .unwrap();
    profile.context.initial_workspace_files = vec!["persist/state.json".into()];
    profile.context.initial_baseline_files = vec!["persist/state.json".into()];
    let run = contract_run(
        "run_zhaowei_agent",
        AgentRunPresentation::Foreground,
        &profile,
    );
    fixture.agent_repository.create_run(&run).await.unwrap();
    let request = chat_request("当前余额100，完成一笔已授权的20元交易");
    let snapshot = json!({ "chatCompletionPayload": request.payload.clone() });
    let (_cancel_sender, mut cancel_receiver) = watch::channel(false);
    execute_agent_loop_with_host_resolver(
        fixture.service.clone(),
        run.id.clone(),
        snapshot,
        request,
        profile.clone(),
        &mut cancel_receiver,
        resolve_chat_commits_and_persistent_state_update(
            fixture.service.clone(),
            fixture.agent_repository.clone(),
            run.id.clone(),
            "0",
            &[],
        ),
    )
    .await
    .unwrap();
    let saved = fixture.agent_repository.load_run(&run.id).await.unwrap();
    assert_eq!(saved.status, AgentRunStatus::Completed);
    let output = fixture
        .agent_repository
        .read_text(&run.id, &WorkspacePath::parse("output/main.md").unwrap())
        .await
        .unwrap();
    assert_eq!(output.text, body);
    let events = read_agent_events(&fixture.agent_repository, &run.id).await;
    for call_id in ["call_private_draft", "call_input_write"] {
        assert!(
            events
                .iter()
                .any(|event| event.event_type == "tool_call_failed"
                    && event.payload["callId"] == call_id
                    && event.payload["errorCode"] == "workspace.path_not_writable")
        );
    }
    let commits = events
        .iter()
        .filter(|event| event.event_type == "chat_commit_completed")
        .collect::<Vec<_>>();
    assert_eq!(
        commits
            .iter()
            .filter(|event| event.payload["isExplicit"] == true)
            .count(),
        1
    );
    assert!(
        commits
            .iter()
            .all(|event| event.payload["path"] == "output/main.md")
    );
    for path in ["plan/draft.md", "input/state.json"] {
        assert!(
            fixture
                .agent_repository
                .read_text(&run.id, &WorkspacePath::parse(path).unwrap())
                .await
                .is_err()
        );
    }
    assert_eq!(
        read_workspace_json(&fixture.agent_repository, &run.id, "persist/state.json").await,
        states[0]
    );
    let initial_version = events
        .iter()
        .find(|event| event.event_type == "persistent_changes_committed")
        .unwrap()
        .payload["stateId"]
        .as_str()
        .unwrap()
        .to_string();
    let mut sessions = vec![format!("{}:inv_root", run.id)];
    for (index, state) in states.iter().enumerate().skip(1) {
        let mut next = contract_run(
            &format!("run_zhaowei_{index}"),
            AgentRunPresentation::Foreground,
            &profile,
        );
        next.workspace_id = run.workspace_id.clone();
        next.stable_chat_id = run.stable_chat_id.clone();
        next.persist_base_state_id = Some(initial_version.clone());
        if index == 2 {
            next.generation_type = "swipe".to_string();
        }
        fixture.agent_repository.create_run(&next).await.unwrap();
        let request = chat_request("从所选回复的持久版本继续");
        let snapshot = json!({ "chatCompletionPayload": request.payload.clone() });
        execute_agent_loop_with_host_resolver(
            fixture.service.clone(),
            next.id.clone(),
            snapshot,
            request,
            profile.clone(),
            &mut cancel_receiver,
            resolve_chat_commits_and_persistent_state_update(
                fixture.service.clone(),
                fixture.agent_repository.clone(),
                next.id.clone(),
                "1",
                &[],
            ),
        )
        .await
        .unwrap();
        assert_eq!(
            fixture
                .agent_repository
                .load_run(&next.id)
                .await
                .unwrap()
                .status,
            AgentRunStatus::Completed
        );
        assert_eq!(
            read_workspace_json(&fixture.agent_repository, &next.id, "persist/state.json").await,
            *state
        );
        assert_eq!(
            read_workspace_json(
                &fixture.agent_repository,
                &next.id,
                "baseline/persist/state.json"
            )
            .await,
            states[0]
        );
        let branch_events = read_agent_events(&fixture.agent_repository, &next.id).await;
        for (id, code) in [
            ("call_failed_json_test", "workspace.json_patch_test_failed"),
            ("call_invalid_batch", "workspace.patch_old_string_not_found"),
            ("call_invalid_json", "workspace.invalid_json"),
            ("call_missing_key", "workspace.json_required_keys"),
            ("call_replace_baseline", "workspace.path_not_writable"),
        ] {
            assert!(
                branch_events
                    .iter()
                    .any(|event| event.event_type == "tool_call_failed"
                        && event.payload["callId"] == id
                        && event.payload["errorCode"] == code)
            );
        }
        assert_eq!(
            branch_events
                .iter()
                .filter(|event| event.event_type == "workspace_patch_applied")
                .count(),
            1
        );
        assert!(
            branch_events
                .iter()
                .filter(|event| event.event_type == "chat_commit_requested")
                .all(|event| event.payload["path"] == "output/main.md")
        );
        let published = branch_events
            .iter()
            .find(|event| event.event_type == "persistent_changes_committed")
            .unwrap();
        assert_eq!(published.payload["baseStateId"], initial_version);
        assert_ne!(published.payload["stateId"], initial_version);
        sessions.push(format!("{}:inv_root", next.id));
    }
    let requests = fixture.model_gateway.requests().await;
    assert!(requests.iter().filter(|request| request.messages.iter().flat_map(|message| &message.parts).any(|part| {
        matches!(part, AgentModelContentPart::Text { text } if text.contains("<workspace-file metadata=") && text.contains("\"balance\": 80") && text.contains("\"fullRead\":true"))
    })).count() >= 2);
    for request in requests {
        assert!(request.tools.iter().all(|tool| tool.tool_id.is_builtin()
            && matches!(
                tool.tool_id.native_name(),
                "agent.list"
                    | "agent.delegate"
                    | "agent.await"
                    | "chat.search"
                    | "chat.read_messages"
                    | "worldinfo.read_activated"
                    | "workspace.list_files"
                    | "workspace.search_files"
                    | "workspace.read_file"
                    | "workspace.write_file"
                    | "workspace.apply_patch"
                    | "workspace.commit"
                    | "workspace.finish"
            )));
    }
    wait_for_closed_sessions(&fixture.model_gateway, sessions).await;
    let _ = fs::remove_dir_all(root).await;
}

#[tokio::test]
async fn agent_runtime_background_run_finish_uses_run_presentation() {
    let root = temp_root("agent-runtime");
    let fixture = agent_runtime_fixture(&root);
    let registry = BuiltinAgentToolRegistry::all();
    let mut profile = fixture
        .profile_service
        .resolve_profile(AgentProfileResolveInput {
            profile_id: None,
            tool_catalog: registry.catalog(),
        })
        .await
        .expect("resolve default profile");
    profile.run.presentation = AgentRunPresentation::Foreground;
    profile.tools.max_rounds = 2;

    let run = AgentRun {
        id: "run_contract".to_string(),
        workspace_id: "stable_contract".to_string(),
        stable_chat_id: "stable_contract".to_string(),
        chat_ref: AgentChatRef::Character {
            character_id: "Alice".to_string(),
            file_name: "Alice.png".to_string(),
        },
        generation_type: "normal".to_string(),
        profile_id: Some(profile.id.as_str().to_string()),
        skill_scope_refs: Default::default(),
        persist_base_state_id: None,
        input_message_count: None,
        presentation: AgentRunPresentation::Background,
        status: AgentRunStatus::Created,
        created_at: Utc::now(),
        updated_at: Utc::now(),
    };
    fixture
        .agent_repository
        .create_run(&run)
        .await
        .expect("create run");

    let request = chat_request("write a short file");
    let prompt_snapshot = json!({ "chatCompletionPayload": request.payload.clone() });
    let (_cancel_sender, mut cancel_receiver) = watch::channel(false);

    fixture
        .service
        .execute_agent_loop_run_inner(
            &run.id,
            prompt_snapshot,
            request,
            profile,
            &mut cancel_receiver,
        )
        .await
        .expect("execute agent loop");

    let saved = fixture
        .agent_repository
        .load_run(&run.id)
        .await
        .expect("load run");
    assert_eq!(saved.status, AgentRunStatus::Completed);
    let artifact = fixture
        .agent_repository
        .read_text(&run.id, &WorkspacePath::parse("output/main.md").unwrap())
        .await
        .expect("read artifact");
    assert_eq!(artifact.text, "hello from real repo");
    let events = fixture
        .agent_repository
        .read_events(
            &run.id,
            AgentRunEventReadQuery {
                event_types: None,
                after_seq: Some(0),
                before_seq: None,
                limit: 100,
                invocation_id: None,
            },
        )
        .await
        .expect("read events");
    let model_completed = events
        .iter()
        .find(|event| event.event_type == "model_completed")
        .expect("model completed event");
    assert_eq!(
        model_completed.payload["modelResponsePath"],
        "model-responses/round-001.json"
    );
    let write_event = events
        .iter()
        .find(|event| event.event_type == "workspace_file_written")
        .expect("workspace file written event");
    assert_eq!(write_event.payload["path"], "output/main.md");
    assert_eq!(write_event.payload["mode"], "replace");
    assert_eq!(write_event.payload["chars"], 20);
    let tool_requested = events
        .iter()
        .find(|event| {
            event.event_type == "tool_call_requested" && event.payload["callId"] == "call_write"
        })
        .expect("tool call requested event");
    assert_eq!(
        tool_requested.payload["toolId"],
        "builtin:workspace.write_file"
    );
    assert_eq!(
        tool_requested.payload["snapshotId"],
        ROOT_AGENT_INVOCATION_ID
    );
    let arguments_ref = tool_requested.payload["argumentsRef"]
        .as_str()
        .expect("arguments ref");
    let arguments = read_workspace_json(&fixture.agent_repository, &run.id, arguments_ref).await;
    assert_eq!(arguments["path"], "output/main.md");
    let result_stored = events
        .iter()
        .find(|event| {
            event.event_type == "tool_result_stored" && event.payload["callId"] == "call_write"
        })
        .expect("tool result stored event");
    assert_eq!(
        result_stored.payload["toolId"],
        "builtin:workspace.write_file"
    );
    let result_ref = result_stored.payload["path"].as_str().expect("result ref");
    let result = read_workspace_json(&fixture.agent_repository, &run.id, result_ref).await;
    assert_eq!(result["toolId"], "builtin:workspace.write_file");
    let tool_snapshot = read_workspace_json(
        &fixture.agent_repository,
        &run.id,
        "input/invocations/inv_root/tool_snapshot.json",
    )
    .await;
    assert_eq!(tool_snapshot["schemaVersion"], 1);
    assert_eq!(tool_snapshot["id"], ROOT_AGENT_INVOCATION_ID);
    assert!(
        tool_snapshot["bindings"]
            .as_array()
            .is_some_and(|bindings| {
                bindings.iter().any(|binding| {
                    binding["descriptor"]["id"] == "builtin:workspace.write_file"
                        && binding["modelAlias"] == "workspace_write_file"
                })
            })
    );
    let context_assembled = events
        .iter()
        .find(|event| event.event_type == "context_assembled")
        .expect("context assembled event");
    assert_eq!(
        context_assembled.payload["toolSnapshotPath"],
        "input/invocations/inv_root/tool_snapshot.json"
    );
    let model_request_created = events
        .iter()
        .find(|event| event.event_type == "model_request_created")
        .expect("model request created event");
    assert_eq!(
        model_request_created.payload["toolTurn"]["snapshotId"],
        ROOT_AGENT_INVOCATION_ID
    );
    assert!(
        events
            .iter()
            .any(|event| event.event_type == "model_response_stored")
    );
    assert!(
        fixture
            .model_gateway
            .requests()
            .await
            .iter()
            .any(|request| request
                .tools
                .iter()
                .any(|tool| tool.tool_id.native_name() == "workspace.write_file"))
    );
    wait_for_closed_sessions(
        &fixture.model_gateway,
        vec!["run_contract:inv_root".to_string()],
    )
    .await;

    let _ = fs::remove_dir_all(root).await;
}

#[tokio::test]
async fn agent_runtime_streaming_keeps_the_existing_final_execution_path() {
    let root = temp_root("agent-runtime-streaming");
    let fixture = agent_runtime_fixture(&root);
    let registry = BuiltinAgentToolRegistry::all();
    let mut definition = fixture
        .profile_service
        .load_profile(DEFAULT_AGENT_PROFILE_ID)
        .await
        .expect("load default profile")
        .expect("default profile exists");
    definition.id = AgentProfileId::parse("streaming-profile").unwrap();
    definition.display_name = "Streaming Profile".to_string();
    definition.run.stream = true;
    fixture
        .profile_service
        .save_profile(definition, registry.catalog())
        .await
        .expect("save streaming profile");
    let profile = fixture
        .profile_service
        .resolve_profile(AgentProfileResolveInput {
            profile_id: Some("streaming-profile"),
            tool_catalog: registry.catalog(),
        })
        .await
        .expect("resolve streaming profile");
    let handle = start_contract_agent_run(
        &fixture,
        &profile,
        AgentRunPresentation::Background,
        "streaming-final",
        None,
    )
    .await;

    let run = wait_for_terminal_agent_run(&fixture.agent_repository, &handle.run_id).await;
    assert_eq!(run.status, AgentRunStatus::Completed);
    assert_eq!(fixture.model_gateway.stream_requests().await, [true, true]);
    if let Some(mut receiver) = fixture
        .service
        .subscribe_live_projection(&handle.run_id)
        .await
        .unwrap()
    {
        tokio::time::timeout(AGENT_CONTRACT_ASYNC_TIMEOUT, async {
            while receiver.changed().await.is_ok() {}
        })
        .await
        .expect("terminal run did not close its live projection");
    }
    assert!(
        fixture
            .service
            .subscribe_live_projection(&handle.run_id)
            .await
            .unwrap()
            .is_none()
    );
    let artifact = fixture
        .agent_repository
        .read_text(
            &handle.run_id,
            &WorkspacePath::parse("output/main.md").unwrap(),
        )
        .await
        .expect("read streamed run artifact");
    assert_eq!(artifact.text, "hello from real repo");

    let _ = fs::remove_dir_all(root).await;
}

#[tokio::test]
async fn agent_runtime_returns_missing_chat_reads_to_the_agent() {
    let root = temp_root("agent-missing-chat-recovery");
    let fixture = agent_runtime_fixture_with_responses(
        &root,
        vec![
            model_tool_response(vec![
                model_tool_call(
                    "call_search_missing_chat",
                    "chat_search",
                    json!({ "query": "context" }),
                ),
                model_tool_call(
                    "call_read_missing_chat",
                    "chat_read_messages",
                    json!({ "messages": [{ "index": 0 }] }),
                ),
            ]),
            model_tool_response(vec![
                model_tool_call(
                    "call_write_after_missing_chat",
                    "workspace_write_file",
                    json!({ "path": "output/main.md", "content": "continued safely" }),
                ),
                model_tool_call(
                    "call_finish_after_missing_chat",
                    "workspace_finish",
                    json!({}),
                ),
            ]),
        ],
    );
    let mut profile = resolve_contract_profile(&fixture).await;
    profile.tools.max_rounds = 2;
    let mut run = contract_run(
        "run_missing_chat_recovery",
        AgentRunPresentation::Background,
        &profile,
    );
    run.input_message_count = Some(1);
    fixture
        .agent_repository
        .create_run(&run)
        .await
        .expect("create run");
    let request = chat_request("continue without the missing chat");
    let prompt_snapshot = json!({ "chatCompletionPayload": request.payload.clone() });
    let (_cancel_sender, mut cancel_receiver) = watch::channel(false);

    fixture
        .service
        .execute_agent_loop_run_inner(
            &run.id,
            prompt_snapshot,
            request,
            profile,
            &mut cancel_receiver,
        )
        .await
        .expect("recoverable chat errors must not stop the run");

    let requests = fixture.model_gateway.requests().await;
    let results = requests[1]
        .messages
        .iter()
        .flat_map(|message| message.parts.iter())
        .filter_map(|part| match part {
            AgentModelContentPart::ToolResult { result } => Some(result),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(results.len(), 2);
    assert!(results.iter().all(|result| result.is_error));
    assert!(
        results
            .iter()
            .all(|result| result.error_code.as_deref() == Some("chat.not_found"))
    );
    assert!(
        results
            .iter()
            .all(|result| result.content.contains("Continue with the context"))
    );

    let saved = fixture
        .agent_repository
        .load_run(&run.id)
        .await
        .expect("load completed run");
    assert_eq!(saved.status, AgentRunStatus::Completed);

    let _ = fs::remove_dir_all(root).await;
}

#[tokio::test]
async fn agent_runtime_normalizes_empty_arguments_and_recovers_from_invalid_arguments() {
    let root = temp_root("agent-tool-arguments");
    let mut malformed = model_tool_call("call_invalid", "workspace_list_files", json!({}));
    malformed["function"]["arguments"] = json!(r#"{"path":"#);
    let fixture = agent_runtime_fixture_with_responses(
        &root,
        vec![
            model_tool_response(vec![
                // The helper encodes this as the wire string "null".
                model_tool_call("call_empty", "workspace_list_files", Value::Null),
                malformed,
            ]),
            model_tool_response(vec![
                model_tool_call("call_over_budget", "workspace_list_files", json!({})),
                model_tool_call(
                    "call_write",
                    "workspace_write_file",
                    json!({ "path": "output/main.md", "content": "continued after invalid arguments" }),
                ),
                model_tool_call("call_finish", "workspace_finish", json!({})),
            ]),
        ],
    );
    let mut profile = resolve_contract_profile(&fixture).await;
    profile.tools.max_rounds = 2;
    profile
        .tools
        .max_calls_per_tool
        .insert(ToolId::builtin("workspace.list_files").unwrap(), 2);
    let run = contract_run("run_arguments", AgentRunPresentation::Background, &profile);
    fixture.agent_repository.create_run(&run).await.unwrap();
    let request = chat_request("list files and write an output");
    let prompt_snapshot = json!({ "chatCompletionPayload": request.payload.clone() });
    let (_cancel_sender, mut cancel_receiver) = watch::channel(false);

    fixture
        .service
        .execute_agent_loop_run_inner(
            &run.id,
            prompt_snapshot,
            request,
            profile,
            &mut cancel_receiver,
        )
        .await
        .expect("argument errors remain recoverable");

    let requests = fixture.model_gateway.requests().await;
    let results = requests[1]
        .messages
        .iter()
        .flat_map(|message| &message.parts)
        .filter_map(|part| match part {
            AgentModelContentPart::ToolResult { result } => Some(result),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(results.len(), 2);
    assert_eq!(results[0].call_id, "call_empty");
    assert!(!results[0].is_error);
    assert!(results[0].structured["entries"].is_array());
    assert_eq!(
        results[1].error_code.as_deref(),
        Some("tool.invalid_arguments")
    );
    assert!(results[1].content.contains(r#"{"path":"#));

    let events = read_agent_events(&fixture.agent_repository, &run.id).await;
    assert!(events.iter().any(|event| {
        event.event_type == "tool_call_failed"
            && event.payload["callId"] == "call_over_budget"
            && event.payload["errorCode"] == "agent.tool_budget_exhausted"
    }));
    let artifact = fixture
        .agent_repository
        .read_text(&run.id, &WorkspacePath::parse("output/main.md").unwrap())
        .await
        .unwrap();
    assert_eq!(artifact.text, "continued after invalid arguments");
    assert_eq!(
        fixture
            .agent_repository
            .load_run(&run.id)
            .await
            .unwrap()
            .status,
        AgentRunStatus::Completed
    );
    fs::remove_dir_all(root).await.unwrap();
}

#[tokio::test]
async fn agent_runtime_duplicate_tool_call_id_preserves_first_audit_facts() {
    let root = temp_root("agent-duplicate-tool-call-id");
    let fixture = agent_runtime_fixture_with_responses(
        &root,
        vec![model_tool_response(vec![
            model_tool_call(
                "duplicate_call",
                "workspace_write_file",
                json!({ "path": "output/first.md", "content": "first" }),
            ),
            model_tool_call(
                "duplicate_call",
                "workspace_write_file",
                json!({ "path": "output/second.md", "content": "second" }),
            ),
        ])],
    );
    let mut profile = resolve_contract_profile(&fixture).await;
    profile.tools.max_rounds = 1;
    let handle = start_contract_agent_run(
        &fixture,
        &profile,
        AgentRunPresentation::Background,
        "duplicate-tool-call-id",
        Some(false),
    )
    .await;

    let run = wait_for_terminal_agent_run(&fixture.agent_repository, &handle.run_id).await;
    assert_eq!(run.status, AgentRunStatus::Failed);
    let events = read_agent_events(&fixture.agent_repository, &handle.run_id).await;
    let requested = events
        .iter()
        .filter(|event| {
            event.event_type == "tool_call_requested" && event.payload["callId"] == "duplicate_call"
        })
        .collect::<Vec<_>>();
    assert_eq!(requested.len(), 1);
    let arguments_ref = requested[0].payload["argumentsRef"]
        .as_str()
        .expect("arguments ref");
    let arguments =
        read_workspace_json(&fixture.agent_repository, &handle.run_id, arguments_ref).await;
    assert_eq!(arguments["path"], "output/first.md");

    let stored_results = events
        .iter()
        .filter(|event| {
            event.event_type == "tool_result_stored" && event.payload["callId"] == "duplicate_call"
        })
        .collect::<Vec<_>>();
    assert_eq!(stored_results.len(), 1);
    let result_ref = stored_results[0].payload["path"]
        .as_str()
        .expect("result ref");
    let result = read_workspace_json(&fixture.agent_repository, &handle.run_id, result_ref).await;
    assert_eq!(result["structured"]["path"], "output/first.md");

    let _ = fs::remove_dir_all(root).await;
}

#[tokio::test]
async fn agent_runtime_agent_list_discovers_callable_profiles_with_real_repositories() {
    let root = temp_root("agent-list");
    let fixture = agent_runtime_fixture_with_responses(
        &root,
        vec![
            json!({
                "choices": [{
                    "message": {
                        "role": "assistant",
                        "content": null,
                        "tool_calls": [{
                            "id": "call_agent_list",
                            "type": "function",
                            "function": {
                                "name": "agent_list",
                                "arguments": "{\"purpose\":\"delegate\"}"
                            }
                        }]
                    }
                }]
            }),
            json!({
                "choices": [{
                    "message": {
                        "role": "assistant",
                        "content": null,
                        "tool_calls": [
                            {
                                "id": "call_write_after_list",
                                "type": "function",
                                "function": {
                                    "name": "workspace_write_file",
                                    "arguments": "{\"path\":\"output/main.md\",\"content\":\"listed agents\"}"
                                }
                            },
                            {
                                "id": "call_finish_after_list",
                                "type": "function",
                                "function": {
                                    "name": "workspace_finish",
                                    "arguments": "{}"
                                }
                            }
                        ]
                    }
                }]
            }),
        ],
    );
    let mut callable = fixture
        .profile_service
        .load_profile("default-writer")
        .await
        .expect("load default profile")
        .expect("default profile exists");
    callable.id = AgentProfileId::parse("scene-editor").expect("profile id");
    callable.display_name = "Scene Editor".to_string();
    callable.description = Some("Edits a draft scene for continuity.".to_string());
    callable.tools.allow.retain(|name| {
        !matches!(
            name.as_str(),
            "builtin:agent.list" | "builtin:agent.delegate" | "builtin:agent.await"
        )
    });
    callable.delegation = AgentDelegationPolicy {
        callable: true,
        allow_as_subagent: true,
        allowed_callers: vec!["default-writer".to_string()],
        description_for_agents: Some("Continuity editor for scene drafts.".to_string()),
        ..Default::default()
    };
    fixture
        .profile_service
        .save_profile(callable, fixture.service.tool_catalog())
        .await
        .expect("save callable profile");
    let mut profile = resolve_contract_profile(&fixture).await;
    profile.run.presentation = AgentRunPresentation::Background;
    profile.tools.max_rounds = 2;
    let run = contract_run(
        "run_agent_list_contract",
        AgentRunPresentation::Background,
        &profile,
    );
    fixture
        .agent_repository
        .create_run(&run)
        .await
        .expect("create run");
    let request = chat_request("list callable agents");
    let prompt_snapshot = json!({ "chatCompletionPayload": request.payload.clone() });
    let (_cancel_sender, mut cancel_receiver) = watch::channel(false);

    fixture
        .service
        .execute_agent_loop_run_inner(
            &run.id,
            prompt_snapshot,
            request,
            profile,
            &mut cancel_receiver,
        )
        .await
        .expect("agent loop");

    let requests = fixture.model_gateway.requests().await;
    let list_results = tool_result_structured_values(&requests[1], "agent.list");
    assert_eq!(list_results.len(), 1);
    assert_eq!(list_results[0]["agents"][0]["profileId"], "scene-editor");
    assert_eq!(
        list_results[0]["agents"][0]["operations"],
        json!(["delegate"])
    );
    assert_eq!(
        list_results[0]["agents"][0]["description"],
        "Continuity editor for scene drafts."
    );

    let _ = fs::remove_dir_all(root).await;
}

#[tokio::test]
async fn agent_runtime_finalize_requires_the_complete_delivered_body_version() {
    for append in [true, false] {
        let root = temp_root("finalize-delivered-version");
        let mut second = vec![model_tool_call(
            "rewrite",
            "workspace_write_file",
            json!({"path":"output/main.md","content":"B"}),
        )];
        if append {
            second.push(model_tool_call(
                "append",
                "workspace_commit",
                json!({"mode":"append"}),
            ));
        }
        let fixture = agent_runtime_fixture_with_responses(
            &root,
            vec![
                model_tool_response(vec![
                    model_tool_call(
                        "body",
                        "workspace_write_file",
                        json!({"path":"output/main.md","content":"A"}),
                    ),
                    model_tool_call("first_commit", "workspace_commit", json!({})),
                ]),
                model_tool_response(second),
                model_tool_response(vec![model_tool_call(
                    "unsafe_finalize",
                    "workspace_write_file",
                    json!({"path":"persist/rejected.json","content":"{}","finalize":true}),
                )]),
                model_tool_response(vec![model_tool_call(
                    "finish",
                    "workspace_finish",
                    json!({}),
                )]),
            ],
        );
        let profile = resolve_contract_profile(&fixture).await;
        let handle = start_contract_agent_run(
            &fixture,
            &profile,
            AgentRunPresentation::Foreground,
            "finalize-delivered-version",
            Some(false),
        )
        .await;
        let (run, resolver) = tokio::join!(
            wait_for_terminal_agent_run(&fixture.agent_repository, &handle.run_id),
            resolve_chat_commits_and_persistent_state_update(
                fixture.service.clone(),
                fixture.agent_repository.clone(),
                handle.run_id.clone(),
                "0",
                &[],
            )
        );
        resolver.unwrap();
        assert_eq!(run.status, AgentRunStatus::Completed);
        let events = read_agent_events(&fixture.agent_repository, &handle.run_id).await;
        assert!(
            events
                .iter()
                .any(|event| event.event_type == "tool_call_failed"
                    && event.payload["callId"] == "unsafe_finalize"
                    && event.payload["errorCode"] == "agent.finalize_not_ready")
        );
        assert!(
            !events
                .iter()
                .any(|event| event.event_type == "chat_commit_requested"
                    && event.payload["callId"] == "unsafe_finalize")
        );
        assert!(
            fixture
                .agent_repository
                .read_text(
                    &handle.run_id,
                    &WorkspacePath::parse("persist/rejected.json").unwrap(),
                )
                .await
                .is_err()
        );
        fs::remove_dir_all(root).await.unwrap();
    }
}

#[tokio::test]
async fn agent_runtime_state_finalize_keeps_saved_mutation_when_host_rejects() {
    let root = temp_root("state-finalize");
    let fixture = agent_runtime_fixture_with_responses(
        &root,
        vec![
            model_tool_response(vec![model_tool_call(
                "premature",
                "workspace_write_file",
                json!({"path":"persist/state.json","content":"{\"n\":99}","finalize":true}),
            )]),
            model_tool_response(vec![
                model_tool_call(
                    "body",
                    "workspace_write_file",
                    json!({"path":"output/main.md","content":"reviewed body"}),
                ),
                model_tool_call(
                    "state",
                    "workspace_write_file",
                    json!({"path":"persist/state.json","content":"{\"n\":0}"}),
                ),
            ]),
            model_tool_response(vec![model_tool_call(
                "invalid",
                "workspace_apply_patch",
                json!({"path":"persist/state.json","json_ops":[{"op":"replace","path":"/missing","value":1}],"finalize":true}),
            )]),
            model_tool_response(vec![
                model_tool_call(
                    "not_sole",
                    "workspace_apply_patch",
                    json!({"path":"persist/state.json","json_ops":[{"op":"replace","path":"/n","value":99}],"finalize":true}),
                ),
                model_tool_call(
                    "read",
                    "workspace_read_file",
                    json!({"path":"persist/state.json"}),
                ),
            ]),
            model_tool_response(vec![model_tool_call(
                "publish_rejected",
                "workspace_apply_patch",
                json!({"path":"persist/state.json","json_ops":[{"op":"test","path":"/n","value":0.0},{"op":"replace","path":"/n","value":1}],"finalize":true}),
            )]),
            model_tool_response(vec![model_tool_call(
                "recover_publish",
                "workspace_commit",
                json!({"finish":true}),
            )]),
        ],
    );
    let mut definition = fixture
        .profile_service
        .load_profile("default-writer")
        .await
        .unwrap()
        .unwrap();
    definition
        .tools
        .max_calls_per_tool
        .insert("builtin:workspace.commit".into(), 2);
    definition
        .tools
        .max_calls_per_tool
        .insert("builtin:workspace.finish".into(), 1);
    fixture
        .profile_service
        .save_profile(definition, fixture.service.tool_catalog())
        .await
        .unwrap();
    let profile = resolve_contract_profile(&fixture).await;
    let handle = start_contract_agent_run(
        &fixture,
        &profile,
        AgentRunPresentation::Foreground,
        "state-finalize",
        Some(false),
    )
    .await;
    let (run, resolver) = tokio::join!(
        wait_for_terminal_agent_run(&fixture.agent_repository, &handle.run_id),
        resolve_chat_commits_and_persistent_state_update(
            fixture.service.clone(),
            fixture.agent_repository.clone(),
            handle.run_id.clone(),
            "0",
            &["publish_rejected"]
        )
    );
    resolver.unwrap();
    assert_eq!(run.status, AgentRunStatus::Completed);
    assert_eq!(
        read_workspace_json(
            &fixture.agent_repository,
            &handle.run_id,
            "persist/state.json"
        )
        .await,
        json!({"n":1})
    );
    let requests = fixture.model_gateway.requests().await;
    assert_eq!(requests.len(), 6);
    assert!(requests.last().unwrap().messages.iter().flat_map(|message| &message.parts).any(|part| matches!(part, AgentModelContentPart::ToolResult {result} if result.call_id=="publish_rejected" && result.is_error && result.structured["savedMutation"]["jsonValidation"]["valid"]==true)));
    let events = read_agent_events(&fixture.agent_repository, &handle.run_id).await;
    assert_eq!(
        events
            .iter()
            .filter(|event| event.event_type == "workspace_file_written"
                && event.payload["path"] == "persist/state.json")
            .count(),
        1
    );
    assert!(!events.iter().any(|event| {
        event.event_type == "chat_commit_requested"
            && ["premature", "invalid", "not_sole"]
                .iter()
                .any(|id| event.payload["callId"] == *id)
    }));
    assert!(
        events
            .iter()
            .any(|event| event.event_type == "persistent_state_metadata_updated")
    );
    fs::remove_dir_all(root).await.unwrap();
}

#[tokio::test]
async fn agent_runtime_commit_finish_requires_last_call_and_host_acceptance() {
    let root = temp_root("agent-commit-finish");
    let fixture = agent_runtime_fixture_with_responses(
        &root,
        vec![
            model_tool_response(vec![
                model_tool_call(
                    "write",
                    "workspace_write_file",
                    json!({ "path": "output/main.md", "content": "reviewed reply" }),
                ),
                model_tool_call("not_last", "workspace_commit", json!({ "finish": true })),
                model_tool_call(
                    "read",
                    "workspace_read_file",
                    json!({ "path": "output/main.md" }),
                ),
                model_tool_call("rejected", "workspace_commit", json!({ "finish": true })),
            ]),
            model_tool_response(vec![model_tool_call(
                "accepted",
                "workspace_commit",
                json!({ "finish": true }),
            )]),
        ],
    );
    let profile = resolve_contract_profile(&fixture).await;
    let handle = start_contract_agent_run(
        &fixture,
        &profile,
        AgentRunPresentation::Foreground,
        "commit-finish",
        Some(false),
    )
    .await;
    let (run, resolver) = tokio::join!(
        wait_for_terminal_agent_run(&fixture.agent_repository, &handle.run_id),
        resolve_chat_commits_and_persistent_state_update(
            fixture.service.clone(),
            fixture.agent_repository.clone(),
            handle.run_id.clone(),
            "0",
            &["rejected"]
        )
    );
    resolver.unwrap();
    assert_eq!(run.status, AgentRunStatus::Completed);
    assert_eq!(fixture.model_gateway.requests().await.len(), 2);
    let events = read_agent_events(&fixture.agent_repository, &handle.run_id).await;
    for (call, code) in [
        ("not_last", "agent.tool_after_finish"),
        ("rejected", "agent.chat_commit_rejected"),
    ] {
        assert!(
            events
                .iter()
                .any(|event| event.event_type == "tool_call_failed"
                    && event.payload["callId"] == call
                    && event.payload["errorCode"] == code)
        );
    }
    assert!(
        !events
            .iter()
            .any(|event| event.event_type == "chat_commit_requested"
                && event.payload["callId"] == "not_last")
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| event.event_type == "chat_commit_completed"
                && event.payload["isExplicit"] == true)
            .count(),
        1
    );
    assert!(
        events
            .iter()
            .any(|event| event.event_type == "persistent_state_metadata_updated")
    );
    fs::remove_dir_all(root).await.unwrap();
}

#[tokio::test]
async fn agent_runtime_foreground_auto_commits_once_per_round_until_explicit_commit() {
    let root = temp_root("agent-foreground");
    let fixture = agent_runtime_fixture_with_responses(
        &root,
        vec![
            model_tool_response(vec![
                model_tool_call(
                    "call_write",
                    "workspace_write_file",
                    json!({ "path": "output/main.md", "content": "foreground answer" }),
                ),
                model_tool_call(
                    "call_patch",
                    "workspace_apply_patch",
                    json!({
                        "path": "output/main.md",
                        "old_string": "foreground answer",
                        "new_string": "revised foreground answer",
                    }),
                ),
                model_tool_call("call_commit", "workspace_commit", json!({})),
                model_tool_call("call_finish_after_auto", "workspace_finish", json!({})),
            ]),
            model_tool_response(vec![
                model_tool_call("call_commit_retry", "workspace_commit", json!({})),
                model_tool_call(
                    "call_write_after_explicit",
                    "workspace_write_file",
                    json!({ "path": "scratch/after.txt", "content": "must not auto commit" }),
                ),
                model_tool_call("call_finish", "workspace_finish", json!({})),
            ]),
        ],
    );
    let mut profile = resolve_contract_profile(&fixture).await;
    profile.run.presentation = AgentRunPresentation::Background;
    profile.tools.max_rounds = 2;
    let run = contract_run(
        "run_foreground_contract",
        AgentRunPresentation::Foreground,
        &profile,
    );
    fixture
        .agent_repository
        .create_run(&run)
        .await
        .expect("create run");
    let request = chat_request("write, revise, commit, and finish");
    let prompt_snapshot = json!({ "chatCompletionPayload": request.payload.clone() });
    let (_cancel_sender, mut cancel_receiver) = watch::channel(false);
    let rejected_commit_calls = ["call_commit"];

    execute_agent_loop_with_host_resolver(
        fixture.service.clone(),
        run.id.clone(),
        prompt_snapshot,
        request,
        profile,
        &mut cancel_receiver,
        resolve_chat_commits_and_persistent_state_update(
            fixture.service.clone(),
            fixture.agent_repository.clone(),
            run.id.clone(),
            "message_1",
            &rejected_commit_calls,
        ),
    )
    .await
    .expect("agent loop");

    let saved = fixture
        .agent_repository
        .load_run(&run.id)
        .await
        .expect("load run");
    assert_eq!(saved.status, AgentRunStatus::Completed);
    let events = read_agent_events(&fixture.agent_repository, &run.id).await;
    let post_auto_guard_failure = events
        .iter()
        .find(|event| {
            event.event_type == "tool_call_failed"
                && event.payload["callId"] == "call_finish_after_auto"
        })
        .expect("automatic commit must not satisfy foreground finish guard");
    assert_eq!(post_auto_guard_failure.level, AgentRunEventLevel::Warn);
    assert_eq!(
        post_auto_guard_failure.payload["errorCode"],
        "agent.foreground_commit_required"
    );
    let commit_requests = events
        .iter()
        .filter(|event| event.event_type == "chat_commit_requested")
        .collect::<Vec<_>>();
    assert_eq!(commit_requests.len(), 3);
    let automatic_commit_request = commit_requests
        .iter()
        .find(|event| event.payload["callId"] == "call_patch")
        .expect("the round must auto-commit only its final text mutation");
    assert_eq!(automatic_commit_request.payload["isExplicit"], false);
    assert_eq!(automatic_commit_request.payload["path"], "output/main.md");
    assert_eq!(automatic_commit_request.payload["mode"], "replace");
    assert!(
        automatic_commit_request.payload["sha256"]
            .as_str()
            .is_some()
    );
    assert!(
        commit_requests
            .iter()
            .all(|event| event.payload["callId"] != "call_write")
    );
    let final_commit_request = commit_requests.last().unwrap();
    assert_eq!(final_commit_request.payload["isExplicit"], true);
    assert_eq!(final_commit_request.payload["runId"], run.id);
    assert_eq!(
        final_commit_request.payload["workspaceId"],
        run.workspace_id
    );
    assert_eq!(
        final_commit_request.payload["stableChatId"],
        run.stable_chat_id
    );
    assert_eq!(final_commit_request.payload["path"], "output/main.md");
    let commit_failures = events
        .iter()
        .filter(|event| event.event_type == "chat_commit_failed")
        .collect::<Vec<_>>();
    assert_eq!(commit_failures.len(), 1);
    assert!(
        commit_failures
            .iter()
            .all(|event| event.level == AgentRunEventLevel::Warn)
    );
    let explicit_commit_failure = events
        .iter()
        .find(|event| {
            event.event_type == "tool_call_failed" && event.payload["callId"] == "call_commit"
        })
        .expect("recoverable explicit commit tool result");
    assert_eq!(
        explicit_commit_failure.payload["errorCode"],
        "agent.chat_commit_rejected"
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| event.event_type == "chat_commit_completed")
            .map(|event| (
                event.payload["callId"].as_str(),
                event.payload["isExplicit"].as_bool(),
                event.payload["messageId"].as_str(),
            ))
            .collect::<Vec<_>>(),
        vec![
            (Some("call_patch"), Some(false), Some("message_1")),
            (Some("call_commit_retry"), Some(true), Some("message_1")),
        ]
    );
    let commit_recorded = events
        .iter()
        .filter(|event| event.event_type == "chat_commit_recorded")
        .collect::<Vec<_>>();
    assert_eq!(commit_recorded.len(), 2);
    assert_eq!(commit_recorded[0].payload["commitCount"], 1);
    assert_eq!(commit_recorded[1].payload["commitCount"], 2);
    let loop_finished = events
        .iter()
        .find(|event| event.event_type == "agent_loop_finished")
        .expect("agent loop finished event");
    assert_eq!(loop_finished.payload["commitCount"], 2);
    let metadata_requested = events
        .iter()
        .find(|event| event.event_type == "persistent_state_metadata_update_requested")
        .expect("persistent metadata update requested");
    assert_eq!(metadata_requested.payload["runId"], run.id);
    assert_eq!(metadata_requested.payload["messageId"], "message_1");
    assert!(metadata_requested.payload["changeCount"].as_u64().is_some());
    assert!(metadata_requested.payload["stateId"].as_str().is_some());
    assert!(events.iter().any(|event| {
        event.event_type == "persistent_state_metadata_updated"
            && event.payload["messageId"] == "message_1"
    }));

    let _ = fs::remove_dir_all(root).await;
}

#[tokio::test]
async fn agent_runtime_streamed_writes_defer_to_patch_and_explicit_commits() {
    let root = temp_root("agent-streamed-write-commit");
    let fixture = agent_runtime_fixture_with_responses(
        &root,
        vec![
            model_tool_response(vec![
                model_tool_call(
                    "call_write_first",
                    "workspace_write_file",
                    json!({ "path": "output/main.md", "content": "draft" }),
                ),
                model_tool_call(
                    "call_patch_first",
                    "workspace_apply_patch",
                    json!({
                        "path": "output/main.md",
                        "old_string": "draft",
                        "new_string": "patched",
                    }),
                ),
            ]),
            model_tool_response(vec![
                model_tool_call(
                    "call_patch_second",
                    "workspace_apply_patch",
                    json!({
                        "path": "output/main.md",
                        "old_string": "patched",
                        "new_string": "second patch",
                    }),
                ),
                model_tool_call(
                    "call_write_second",
                    "workspace_write_file",
                    json!({
                        "path": "output/main.md",
                        "content": "streamed replacement",
                    }),
                ),
            ]),
            model_tool_response(vec![
                model_tool_call("call_commit", "workspace_commit", json!({})),
                model_tool_call("call_finish", "workspace_finish", json!({})),
            ]),
        ],
    );
    let mut profile = resolve_contract_profile(&fixture).await;
    profile.tools.max_rounds = 3;
    let handle = start_contract_agent_run(
        &fixture,
        &profile,
        AgentRunPresentation::Foreground,
        "streamed-write-commit",
        Some(true),
    )
    .await;

    let (run, resolver) = tokio::join!(
        wait_for_terminal_agent_run(&fixture.agent_repository, &handle.run_id),
        resolve_chat_commits_and_persistent_state_update(
            fixture.service.clone(),
            fixture.agent_repository.clone(),
            handle.run_id.clone(),
            "message_streamed",
            &[],
        ),
    );
    resolver.expect("host resolver");
    assert_eq!(run.status, AgentRunStatus::Completed);

    let events = read_agent_events(&fixture.agent_repository, &handle.run_id).await;
    let commits = events
        .iter()
        .filter(|event| event.event_type == "chat_commit_requested")
        .collect::<Vec<_>>();
    assert_eq!(commits.len(), 2);
    assert_eq!(commits[0].payload["callId"], "call_patch_first");
    assert_eq!(commits[0].payload["isExplicit"], false);
    assert_eq!(commits[1].payload["callId"], "call_commit");
    assert_eq!(commits[1].payload["isExplicit"], true);
    assert!(commits.iter().all(|event| {
        event.payload["callId"] != "call_write_first"
            && event.payload["callId"] != "call_write_second"
            && event.payload["callId"] != "call_patch_second"
    }));

    let _ = fs::remove_dir_all(root).await;
}

#[tokio::test]
async fn agent_runtime_retries_retryable_model_errors_with_real_repositories() {
    let root = temp_root("agent-retry");
    let fixture = agent_runtime_fixture_with_results(
        &root,
        vec![
            Err(ApplicationError::UpstreamFailure(UpstreamFailure::network(
                UPSTREAM_NETWORK_TIMEOUT,
                None,
                "tauritavern.error.network.timeout",
            ))),
            Ok(json!({
                "choices": [{
                    "message": {
                        "role": "assistant",
                        "content": null,
                        "tool_calls": [{
                            "id": "call_write",
                            "type": "function",
                            "function": {
                                "name": "workspace_write_file",
                                "arguments": "{\"path\":\"output/main.md\",\"content\":\"retry succeeded\"}"
                            }
                        }]
                    }
                }]
            })),
            Ok(json!({
                "choices": [{
                    "message": {
                        "role": "assistant",
                        "content": null,
                        "tool_calls": [{
                            "id": "call_finish",
                            "type": "function",
                            "function": {
                                "name": "workspace_finish",
                                "arguments": "{}"
                            }
                        }]
                    }
                }]
            })),
        ],
    );
    let mut profile = resolve_contract_profile(&fixture).await;
    profile.run.presentation = AgentRunPresentation::Background;
    profile.run.model_retry.max_retries = 1;
    profile.run.model_retry.interval_ms = 1;
    profile.tools.max_rounds = 2;
    let run = contract_run(
        "run_retry_contract",
        AgentRunPresentation::Background,
        &profile,
    );
    fixture
        .agent_repository
        .create_run(&run)
        .await
        .expect("create run");
    let request = chat_request("write with retry");
    let prompt_snapshot = json!({ "chatCompletionPayload": request.payload.clone() });
    let (_cancel_sender, mut cancel_receiver) = watch::channel(false);

    fixture
        .service
        .execute_agent_loop_run_inner(
            &run.id,
            prompt_snapshot,
            request,
            profile,
            &mut cancel_receiver,
        )
        .await
        .expect("agent loop");

    assert_eq!(fixture.model_gateway.requests().await.len(), 3);
    let artifact = fixture
        .agent_repository
        .read_text(&run.id, &WorkspacePath::parse("output/main.md").unwrap())
        .await
        .expect("read artifact");
    assert_eq!(artifact.text, "retry succeeded");
    let events = read_agent_events(&fixture.agent_repository, &run.id).await;
    let retry_failed = events
        .iter()
        .find(|event| event.event_type == "model_call_attempt_failed")
        .expect("retryable failed attempt");
    assert_eq!(retry_failed.payload["attempt"], 1);
    assert_eq!(retry_failed.payload["maxRetries"], 1);
    assert_eq!(retry_failed.payload["retryable"], true);
    assert_eq!(retry_failed.payload["willRetry"], true);
    let retry_scheduled = events
        .iter()
        .find(|event| event.event_type == "model_call_retry_scheduled")
        .expect("retry scheduled event");
    assert_eq!(retry_scheduled.payload["nextAttempt"], 2);
    assert_eq!(retry_scheduled.payload["intervalMs"], 1);

    let _ = fs::remove_dir_all(root).await;
}

#[tokio::test]
async fn agent_runtime_does_not_retry_non_retryable_model_errors() {
    let root = temp_root("agent-no-retry");
    let fixture = agent_runtime_fixture_with_results(
        &root,
        vec![Err(ApplicationError::ValidationError(
            "model.invalid_tool_call: missing id".to_string(),
        ))],
    );
    let mut profile = resolve_contract_profile(&fixture).await;
    profile.run.model_retry.max_retries = 2;
    profile.run.model_retry.interval_ms = 1;
    let run = contract_run(
        "run_no_retry_contract",
        AgentRunPresentation::Background,
        &profile,
    );
    fixture
        .agent_repository
        .create_run(&run)
        .await
        .expect("create run");
    let request = chat_request("write without retry");
    let prompt_snapshot = json!({ "chatCompletionPayload": request.payload.clone() });
    let (_cancel_sender, mut cancel_receiver) = watch::channel(false);

    let error = fixture
        .service
        .execute_agent_loop_run_inner(
            &run.id,
            prompt_snapshot,
            request,
            profile,
            &mut cancel_receiver,
        )
        .await
        .expect_err("non-retryable error");

    assert!(error.to_string().contains("missing id"));
    assert_eq!(fixture.model_gateway.requests().await.len(), 1);
    let events = read_agent_events(&fixture.agent_repository, &run.id).await;
    assert!(
        events
            .iter()
            .all(|event| event.event_type != "model_call_retry_scheduled")
    );

    let _ = fs::remove_dir_all(root).await;
}

#[tokio::test]
async fn agent_runtime_replays_frozen_macros_before_reading_and_searching() {
    use tt_domain::models::chat::ChatMessage;
    use tt_domain::models::skill::{
        SkillImportInput, SkillInlineFile, SkillInstallRequest, SkillScope,
    };
    use tt_ports::repositories::skill_repository::SkillRepository;

    let root = temp_root("frozen-macros");
    let source = "heading\n{{description}}\n{{char}}\n{{greeting::1}} / {{getvar::Name}} / {{getglobalvar::Name}} / {{outlet::Lore}} / {{date}} / {{instructUserPrefix}}";
    let fixture = agent_runtime_fixture_with_responses(
        &root,
        vec![
            model_tool_response(vec![
                model_tool_call(
                    "write",
                    "workspace_write_file",
                    json!({ "path": "output/main.md", "content": source }),
                ),
                model_tool_call(
                    "skill_read",
                    "skill_read",
                    json!({ "name": "macro-demo", "path": "references/template.md", "start_line": 3, "line_count": 1 }),
                ),
                model_tool_call(
                    "skill_search",
                    "skill_search",
                    json!({ "name": "macro-demo", "query": "lantern" }),
                ),
                model_tool_call(
                    "chat_read",
                    "chat_read_messages",
                    json!({ "messages": [{ "index": 0, "start_line": 3, "line_count": 1 }] }),
                ),
                model_tool_call(
                    "chat_search",
                    "chat_search",
                    json!({ "query": "lantern", "read_matches": true }),
                ),
                model_tool_call(
                    "chat_full",
                    "chat_read_messages",
                    json!({ "messages": [{"index":0}] }),
                ),
                model_tool_call(
                    "file_read",
                    "workspace_read_file",
                    json!({ "path": "output/main.md" }),
                ),
                model_tool_call(
                    "file_search",
                    "workspace_search_files",
                    json!({ "path": "output", "query": "lantern" }),
                ),
                model_tool_call(
                    "script",
                    "skill_run_script",
                    json!({ "skill": "macro-demo", "script": "helper" }),
                ),
            ]),
            model_tool_response(vec![model_tool_call(
                "finish",
                "workspace_finish",
                json!({}),
            )]),
        ],
    );
    FileSkillRepository::new(root.join("_tauritavern/skills"))
        .install_import(SkillInstallRequest {
            target_scope: SkillScope::Global,
            conflict_strategy: None,
            input: SkillImportInput::InlineFiles {
                source: json!({ "kind": "test" }),
                files: [
                    ("SKILL.md", "---\nname: macro-demo\ndescription: Macro test\n---\n{{char}}"),
                    ("references/template.md", source),
                    ("scripts/helper.js", "import { macros, workspace } from '@tauritavern/runtime'; export default () => macros.render(workspace.readText('output/main.md'));"),
                ].into_iter().map(|(path, content)| SkillInlineFile {
                    path: path.into(), content: content.into(), encoding: "utf8".into(),
                    media_type: None, size_bytes: None, sha256: None,
                }).collect(),
            },
        }).await.unwrap();
    let mut profile = resolve_contract_profile(&fixture).await;
    profile.tools.max_rounds = 2;
    profile.tools.max_calls_per_run = 20;
    let mut run = contract_run("run_macros", AgentRunPresentation::Background, &profile);
    run.chat_ref = AgentChatRef::Character {
        character_id: "Alice".into(),
        file_name: "macros.jsonl".into(),
    };
    run.input_message_count = Some(1);
    let mut chat = Chat::new("User", "Alice");
    chat.file_name = Some("macros.jsonl".into());
    chat.add_message(ChatMessage::user("User", source));
    fixture.chat_repository.save(&chat).await.unwrap();
    fixture.agent_repository.create_run(&run).await.unwrap();
    let request = chat_request("read the frozen text");
    let prompt = json!({
        "chatCompletionPayload": request.payload,
        "worldInfoActivation": { "entries": [] },
        "frozenRunInputSnapshot": {
            "promptInputs": {
                "extensionPrompts": { "customWIOutlet_Lore": { "value": "Outlet" } }
            },
            "macroContext": {
                "names": { "char": "Frozen name" },
                "character": {
                    "description": "first line\nblue lantern\nlast line",
                    "firstMessage": "Main",
                    "alternateGreetings": ["Alternate"]
                },
                "variableValues": { "local": { "Name": "Local" }, "global": { "Name": "Global" } },
                "builtins": { "date": "2026-09-05", "instructInput": "USER:" }
            }
        }
    });
    let (_cancel, mut receiver) = watch::channel(false);
    fixture
        .service
        .execute_agent_loop_run_inner(&run.id, prompt, request, profile, &mut receiver)
        .await
        .unwrap();
    let requests = fixture.model_gateway.requests().await;
    let results = requests[1]
        .messages
        .iter()
        .flat_map(|message| &message.parts)
        .filter_map(|part| match part {
            AgentModelContentPart::ToolResult { result } => Some((result.call_id.as_str(), result)),
            _ => None,
        })
        .collect::<std::collections::HashMap<_, _>>();
    for result in results.values() {
        assert!(!result.is_error, "{}: {}", result.call_id, result.content);
    }
    assert!(results["skill_read"].content.contains("3 | blue lantern"));
    assert_eq!(results["skill_read"].structured["totalLines"], 6);
    assert_eq!(
        results["chat_read"].structured["messages"][0]["text"],
        "blue lantern"
    );
    assert_eq!(
        results["chat_search"].structured["messageReads"],
        results["chat_full"].structured
    );
    assert!(
        results["chat_search"]
            .content
            .contains(&results["chat_full"].content)
    );
    for id in ["skill_search", "chat_search"] {
        assert_eq!(
            results[id].structured["hits"].as_array().unwrap().len(),
            1,
            "{id}"
        );
    }
    assert!(
        results["file_search"].structured["hits"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert!(results["file_read"].content.contains("{{description}}"));
    assert_eq!(results["file_read"].structured["totalLines"], 4);
    assert!(results["script"].content.contains("blue lantern"));
    assert!(results["script"].content.contains("Frozen name"));
    assert!(
        results["script"]
            .content
            .contains("Alternate / Local / Global / Outlet / 2026-09-05 / USER:")
    );
    assert_eq!(
        fixture
            .agent_repository
            .read_text(&run.id, &WorkspacePath::parse("output/main.md").unwrap())
            .await
            .unwrap()
            .text,
        source
    );
    let _ = fs::remove_dir_all(root).await;
}
