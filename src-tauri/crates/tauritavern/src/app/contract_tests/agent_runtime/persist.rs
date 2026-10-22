use super::*;

#[tokio::test]
async fn event_archive_rejects_incompatible_state_writes_before_publication() {
    use tt_domain::models::agent::event_archive::EventArchivePolicy;

    let root = temp_root("event-archive-write-validation");
    let valid = r#"{"recentEvents":[{"id":"E1","detail":"retained"}],"archives":[],"balance":123456789012345678901,"extra":{"keep":true}}"#;
    let legacy = r#"{"current":{"location":"street"},"turn_events":["retained"]}"#;
    let invalid_identity = r#"{"recentEvents":[{"id":1}],"archives":[]}"#;
    let oversized_duplicate_history = json!({
        "recentEvents": vec![json!({"id":"E1"}); 33], "archives": []
    })
    .to_string();
    let mut responses = vec![
        model_tool_response(vec![
            model_tool_call(
                "alias_create",
                "workspace_write_file",
                json!({"path":"persist/STATE.JSON","content":valid,"create_only":true}),
            ),
            model_tool_call(
                "bad_state",
                "workspace_write_file",
                json!({"path":"persist/state.json","content":legacy,"create_only":true}),
            ),
        ]),
        model_tool_response(vec![
            model_tool_call(
                "valid_state",
                "workspace_write_file",
                json!({"path":"persist/state.json","content":valid,"create_only":true}),
            ),
            model_tool_call(
                "legacy_source",
                "workspace_write_file",
                json!({"path":"output/legacy.json","content":legacy}),
            ),
            model_tool_call(
                "bad_copy",
                "workspace_write_file",
                json!({"path":"persist/state.json","source_path":"output/legacy.json"}),
            ),
        ]),
        model_tool_response(vec![model_tool_call(
            "bad_patch",
            "workspace_apply_patch",
            json!({"path":"persist/state.json","json_ops":[
                {"op":"replace","path":"/recentEvents","value":{}}
            ]}),
        )]),
        model_tool_response(vec![
            model_tool_call(
                "missing_id",
                "workspace_write_file",
                json!({"path":"persist/state.json","content":r#"{"recentEvents":[{"summary":"keep this event"}],"archives":[]}"#}),
            ),
            model_tool_call(
                "identity_source",
                "workspace_write_file",
                json!({"path":"output/invalid-identity.json","content":invalid_identity}),
            ),
            model_tool_call(
                "non_string_id_copy",
                "workspace_write_file",
                json!({"path":"persist/state.json","source_path":"output/invalid-identity.json"}),
            ),
            model_tool_call(
                "duplicate_id_patch",
                "workspace_apply_patch",
                json!({"path":"persist/state.json","json_ops":[
                    {"op":"add","path":"/recentEvents/-","value":{"id":"E1"}}
                ]}),
            ),
            model_tool_call(
                "duplicate_ids_above_threshold",
                "workspace_write_file",
                json!({"path":"persist/state.json","content":oversized_duplicate_history}),
            ),
            model_tool_call(
                "malformed_index",
                "workspace_apply_patch",
                json!({"path":"persist/state.json","json_ops":[
                    {"op":"add","path":"/archives/-","value":{"kind":"agent.eventArchive","eventIds":{}}}
                ]}),
            ),
            model_tool_call(
                "invalid_index_path",
                "workspace_apply_patch",
                json!({"path":"persist/state.json","json_ops":[
                    {"op":"add","path":"/archives/-","value":{
                        "kind":"agent.eventArchive","eventIds":["E0"],
                        "path":"persist/history/../old.json","sha256":"0".repeat(64)
                    }}
                ]}),
            ),
            model_tool_call(
                "archived_id_overlap",
                "workspace_apply_patch",
                json!({"path":"persist/state.json","json_ops":[
                    {"op":"add","path":"/archives/-","value":{
                        "kind":"agent.eventArchive","eventIds":["E1"],
                        "path":"persist/history/old.json","sha256":"0".repeat(64)
                    }}
                ]}),
            ),
        ]),
        model_tool_response(vec![
            model_tool_call(
                "read_preserved",
                "workspace_read_file",
                json!({"path":"persist/state.json"}),
            ),
            model_tool_call("finish", "workspace_finish", json!({})),
        ]),
    ];
    let mut alias_calls = Vec::new();
    // Read the real file through its alias so the normal read/SHA guard also
    // succeeds on Windows; rejection must come from the state-path policy.
    if cfg!(windows) {
        alias_calls.push(model_tool_call(
            "alias_read",
            "workspace_read_file",
            json!({"path":"persist/STATE.JSON"}),
        ));
    }
    alias_calls.extend([
        model_tool_call(
            "alias_replace",
            "workspace_write_file",
            json!({"path":"persist/STATE.JSON","content":legacy}),
        ),
        model_tool_call(
            "alias_copy",
            "workspace_write_file",
            json!({"path":"persist/STATE.JSON","source_path":"output/legacy.json"}),
        ),
        model_tool_call(
            "alias_append",
            "workspace_write_file",
            json!({"path":"persist/STATE.JSON","mode":"append","content":"not JSON"}),
        ),
        model_tool_call(
            "state_append",
            "workspace_write_file",
            json!({"path":"persist/state.json","mode":"append","content":"not JSON"}),
        ),
    ]);
    if cfg!(windows) {
        alias_calls.push(model_tool_call(
            "alias_patch",
            "workspace_apply_patch",
            json!({"path":"persist/STATE.JSON","json_ops":[
                {"op":"replace","path":"/recentEvents","value":{}}
            ]}),
        ));
    }
    responses.insert(responses.len() - 1, model_tool_response(alias_calls));
    // These names alias the canonical file (or its parent) on Windows.
    // Reject them before IO on all platforms, including append without a read.
    let mut invalid_path_calls = Vec::new();
    let mut invalid_path_ids = Vec::new();
    for (index, path) in [
        "persist/state.json.",
        "persist/state.json. ",
        "persist./state.json",
        "persist /state.json",
        "persist/state.json::$DATA",
        "persist/state.json:stream",
    ]
    .iter()
    .enumerate()
    {
        for (operation, tool, args) in [
            (
                "replace",
                "workspace_write_file",
                json!({"path":path,"content":legacy}),
            ),
            (
                "copy",
                "workspace_write_file",
                json!({"path":path,"source_path":"output/legacy.json"}),
            ),
            (
                "append",
                "workspace_write_file",
                json!({"path":path,"mode":"append","content":"not JSON"}),
            ),
            (
                "patch",
                "workspace_apply_patch",
                json!({"path":path,"json_ops":[{"op":"replace","path":"/recentEvents","value":{}}]}),
            ),
        ] {
            let id = format!("invalid_path_{index}_{operation}");
            invalid_path_calls.push(model_tool_call(&id, tool, args));
            invalid_path_ids.push(id);
        }
    }
    responses.insert(responses.len() - 1, model_tool_response(invalid_path_calls));
    let fixture = agent_runtime_fixture_with_responses(&root, responses);
    let default_profile = resolve_contract_profile(&fixture).await;
    let mut definition = fixture
        .profile_service
        .load_profile(default_profile.id.as_str())
        .await
        .unwrap()
        .unwrap();
    definition.context.event_archive = Some(EventArchivePolicy {
        max_recent_events: 32,
        keep_recent_events: 16,
    });
    fixture
        .profile_service
        .save_profile(definition, fixture.service.tool_catalog())
        .await
        .unwrap();
    let profile = fixture
        .profile_service
        .resolve_profile(AgentProfileResolveInput {
            profile_id: Some(default_profile.id.as_str()),
            tool_catalog: fixture.service.tool_catalog(),
        })
        .await
        .unwrap();
    let handle = start_contract_agent_run(
        &fixture,
        &profile,
        AgentRunPresentation::Background,
        "archive-write-validation",
        Some(false),
    )
    .await;
    super::resume::wait_for_checkpoint(&fixture, &handle.run_id).await;
    let run = fixture
        .agent_repository
        .load_run(&handle.run_id)
        .await
        .unwrap();
    assert_eq!(run.status, AgentRunStatus::Completed);
    let events = read_agent_events(&fixture.agent_repository, &handle.run_id).await;
    for call_id in invalid_path_ids {
        let rejected = events
            .iter()
            .find(|event| {
                event.event_type == "tool_call_failed" && event.payload["callId"] == call_id
            })
            .expect("ambiguous paths must be rejected before writing");
        assert_eq!(rejected.payload["errorCode"], "workspace.invalid_path");
    }
    let mut rejected_calls = vec![
        "alias_create",
        "alias_replace",
        "alias_copy",
        "alias_append",
        "state_append",
        "bad_state",
        "bad_copy",
        "bad_patch",
        "missing_id",
        "non_string_id_copy",
        "duplicate_id_patch",
        "duplicate_ids_above_threshold",
        "malformed_index",
        "invalid_index_path",
        "archived_id_overlap",
    ];
    if cfg!(windows) {
        rejected_calls.push("alias_patch");
    }
    for call_id in rejected_calls {
        let rejected = events
            .iter()
            .find(|event| {
                event.event_type == "tool_call_failed" && event.payload["callId"] == call_id
            })
            .expect("invalid state is returned as a recoverable tool error");
        assert_eq!(rejected.payload["errorCode"], "agent.event_archive_invalid");
    }
    let state_path = WorkspacePath::parse("persist/state.json").unwrap();
    assert_eq!(
        fixture
            .agent_repository
            .read_text(&handle.run_id, &state_path)
            .await
            .unwrap()
            .text,
        valid
    );
    let published = events
        .iter()
        .find(|event| event.event_type == "persistent_changes_committed")
        .unwrap();
    let state_id = published.payload["stateId"].as_str().unwrap();
    assert_eq!(
        fixture
            .agent_repository
            .read_persistent_file(&run.workspace_id, state_id, &state_path)
            .await
            .unwrap()
            .text,
        valid
    );
    fs::remove_dir_all(root).await.unwrap();
}

#[tokio::test]
async fn event_archival_preserves_history_versions_and_run_baselines() {
    use tt_domain::models::agent::event_archive::EventArchivePolicy;
    async fn select_state(fixture: &AgentRuntimeFixture, run: &AgentRun, state_id: &str) {
        use tt_ports::repositories::chat_payload_commit_repository::{
            ChatPayloadCommitRepository, ChatPayloadTarget,
        };
        let header = json!({"user_name":"User","character_name":"Alice","chat_metadata":{"integrity":run.stable_chat_id}});
        let message = json!({"name":"Alice","is_user":false,"mes":"Existing reply","extra":{"tauritavern":{"agent":{"runId":run.id,"persistStateStatus":"committed","persistStateId":state_id}}}});
        let bytes = format!("{header}\n{message}\n").into_bytes();
        let session = fixture
            .chat_repository
            .begin(
                ChatPayloadTarget::Character {
                    character_id: "Alice".into(),
                    file_name: "archive.jsonl".into(),
                },
                true,
                None,
            )
            .await
            .unwrap();
        fixture
            .chat_repository
            .append(&session.session_id, 0, &bytes)
            .await
            .unwrap();
        fixture
            .chat_repository
            .finish(&session.session_id, bytes.len() as u64)
            .await
            .unwrap();
    }
    let root = temp_root("event-archive");
    let source = r#"{"schemaVersion":1,"revision":7,"player":{"balance":123456789012345678901},"threads":[{"promise":"keep"}],"unknown":{"keep":true},"recentEvents":[{"id":"E3","amount":0.123456789012345678901},{"id":"E1","extra":{"keep":true}},{"id":"E2","participants":["Alice"]},{"id":"E4"},{"id":"E5"}],"archives":[{"path":"persist/history/manual.json","note":"keep"}]}"#;
    let fixture = agent_runtime_fixture_with_responses(
        &root,
        vec![
            model_tool_response(vec![
                model_tool_call(
                    "seed",
                    "workspace_write_file",
                    json!({"path":"persist/state.json","content":source}),
                ),
                model_tool_call(
                    "legacy",
                    "workspace_write_file",
                    json!({"path":"persist/history/manual.json","content":"{\"legacy\":true}"}),
                ),
                model_tool_call("seed_finish", "workspace_finish", json!({})),
            ]),
            model_tool_response(vec![model_tool_call(
                "archive_finish",
                "workspace_finish",
                json!({}),
            )]),
            model_tool_response(vec![model_tool_call(
                "next_finish",
                "workspace_finish",
                json!({}),
            )]),
            model_tool_response(vec![model_tool_call(
                "second_archive_finish",
                "workspace_finish",
                json!({}),
            )]),
        ],
    );
    let profile = resolve_contract_profile(&fixture).await;
    let seed = start_contract_agent_run(
        &fixture,
        &profile,
        AgentRunPresentation::Background,
        "archive",
        Some(false),
    )
    .await;
    super::resume::wait_for_checkpoint(&fixture, &seed.run_id).await;
    let seed_run = fixture
        .agent_repository
        .load_run(&seed.run_id)
        .await
        .unwrap();
    let seed_events = read_agent_events(&fixture.agent_repository, &seed.run_id).await;
    let base_id = seed_events
        .iter()
        .find(|event| event.event_type == "persistent_changes_committed")
        .unwrap()
        .payload["stateId"]
        .as_str()
        .unwrap()
        .to_owned();
    let mut definition = fixture
        .profile_service
        .load_profile(profile.id.as_str())
        .await
        .unwrap()
        .unwrap();
    definition.context.event_archive = Some(EventArchivePolicy {
        max_recent_events: 3,
        keep_recent_events: 2,
    });
    definition.context.initial_workspace_files = vec!["persist/state.json".into()];
    definition.context.initial_baseline_files = vec!["persist/state.json".into()];
    fixture
        .profile_service
        .save_profile(definition, fixture.service.tool_catalog())
        .await
        .unwrap();
    let profile = fixture
        .profile_service
        .resolve_profile(AgentProfileResolveInput {
            profile_id: Some(profile.id.as_str()),
            tool_catalog: fixture.service.tool_catalog(),
        })
        .await
        .unwrap();
    let request = |state_id: &str| {
        serde_json::from_value::<AgentStartRunDto>(json!({
        "chatRef":seed_run.chat_ref,"stableChatId":seed_run.stable_chat_id,"profileId":profile.id,
        "generationType":"normal","persistBaseStateId":state_id,
        "promptSnapshot":{"contextPolicy":profile.context,"chatCompletionPayload":chat_request("archive").payload},
        "options":{"presentation":"background","stream":false}
    })).unwrap()
    };
    select_state(&fixture, &seed_run, &base_id).await;
    let handle = fixture.service.start_run(request(&base_id)).await.unwrap();
    let checkpoint = super::resume::wait_for_checkpoint(&fixture, &handle.run_id).await;
    assert_eq!(checkpoint.next_step, "finished");
    let events = read_agent_events(&fixture.agent_repository, &handle.run_id).await;
    let archival = events
        .iter()
        .find(|event| event.event_type == "memory_events_archived")
        .expect("automatic archive");
    assert!(
        archival.seq
            < events
                .iter()
                .find(|event| event.event_type == "model_request_created")
                .unwrap()
                .seq
    );
    let archive_path = WorkspacePath::parse(archival.payload["path"].as_str().unwrap()).unwrap();
    let requests = fixture.model_gateway.requests().await;
    let supplied = requests[1]
        .messages
        .iter()
        .flat_map(|message| &message.parts)
        .filter_map(|part| match part {
            AgentModelContentPart::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(supplied.contains(archive_path.as_str()));
    assert!(supplied.contains("123456789012345678901"));
    assert!(!supplied.contains("0.123456789012345678901"));
    let archive = fixture
        .agent_repository
        .read_text(&handle.run_id, &archive_path)
        .await
        .unwrap();
    let state_path = WorkspacePath::parse("persist/state.json").unwrap();
    let compact = fixture
        .agent_repository
        .read_text(&handle.run_id, &state_path)
        .await
        .unwrap();
    let baseline = fixture
        .agent_repository
        .read_text(
            &handle.run_id,
            &WorkspacePath::parse("baseline/persist/state.json").unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(baseline.text, compact.text);
    let original: Value = tt_domain::json_input::from_str(source).unwrap();
    let mut current: Value = tt_domain::json_input::from_str(&compact.text).unwrap();
    let archived: Value = tt_domain::json_input::from_str(&archive.text).unwrap();
    let mut reconstructed = archived["events"].as_array().unwrap().clone();
    reconstructed.extend(current["recentEvents"].as_array().unwrap().iter().cloned());
    assert_eq!(Value::Array(reconstructed), original["recentEvents"]);
    assert_eq!(current["recentEvents"].as_array().unwrap().len(), 2);
    assert_eq!(current["archives"][0], original["archives"][0]);
    assert_eq!(current["archives"][1]["sha256"], archive.sha256);
    assert_eq!(
        current["player"]["balance"].to_string(),
        "123456789012345678901"
    );
    assert_eq!(
        archived["events"][0]["amount"].to_string(),
        "0.123456789012345678901"
    );
    current["recentEvents"] = original["recentEvents"].clone();
    current["archives"] = original["archives"].clone();
    assert_eq!(current, original);
    assert_eq!(
        fixture
            .agent_repository
            .read_persistent_file(&seed_run.workspace_id, &base_id, &state_path)
            .await
            .unwrap()
            .text,
        source
    );
    let new_id = events
        .iter()
        .find(|event| event.event_type == "persistent_changes_committed")
        .unwrap()
        .payload["stateId"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        fixture
            .agent_repository
            .read_persistent_file(&seed_run.workspace_id, &new_id, &archive_path)
            .await
            .unwrap()
            .text,
        archive.text
    );
    select_state(&fixture, &seed_run, &new_id).await;
    let next = fixture.service.start_run(request(&new_id)).await.unwrap();
    super::resume::wait_for_checkpoint(&fixture, &next.run_id).await;
    let next_events = read_agent_events(&fixture.agent_repository, &next.run_id).await;
    assert!(
        !next_events
            .iter()
            .any(|event| event.event_type == "memory_events_archived")
    );
    assert_eq!(
        fixture
            .agent_repository
            .read_text(&next.run_id, &state_path)
            .await
            .unwrap()
            .text,
        compact.text
    );
    let mut growing: Value = tt_domain::json_input::from_str(&compact.text).unwrap();
    growing["recentEvents"]
        .as_array_mut()
        .unwrap()
        .extend([json!({"id":"E6"}), json!({"id":"E7"})]);
    fixture
        .agent_repository
        .write_text(
            &next.run_id,
            &state_path,
            &serde_json::to_string_pretty(&growing).unwrap(),
        )
        .await
        .unwrap();
    let growing_version = fixture
        .agent_repository
        .commit_persistent_changes(&next.run_id, Some(&new_id))
        .await
        .unwrap();
    select_state(&fixture, &seed_run, &growing_version.state_id).await;
    let again = fixture
        .service
        .start_run(request(&growing_version.state_id))
        .await
        .unwrap();
    assert_eq!(
        super::resume::wait_for_checkpoint(&fixture, &again.run_id)
            .await
            .next_step,
        "finished"
    );
    let second_text = fixture
        .agent_repository
        .read_text(&again.run_id, &state_path)
        .await
        .unwrap()
        .text;
    let second: Value = tt_domain::json_input::from_str(&second_text).unwrap();
    assert_eq!(second["archives"].as_array().unwrap().len(), 3);
    assert_eq!(second["archives"][2]["eventIds"], json!(["E4", "E5"]));
    assert_eq!(second["recentEvents"], json!([{"id":"E6"},{"id":"E7"}]));
    assert_eq!(
        fixture
            .agent_repository
            .read_text(&again.run_id, &archive_path)
            .await
            .unwrap()
            .text,
        archive.text
    );
    fixture
        .agent_repository
        .write_text(&seed.run_id, &archive_path, "conflicting archive")
        .await
        .unwrap();
    let conflicting = fixture
        .agent_repository
        .commit_persistent_changes(&seed.run_id, Some(&base_id))
        .await
        .unwrap();
    let calls_before = fixture.model_gateway.requests().await.len();
    select_state(&fixture, &seed_run, &conflicting.state_id).await;
    let rejected = fixture
        .service
        .start_run(request(&conflicting.state_id))
        .await
        .unwrap();
    let rejected_run =
        wait_for_terminal_agent_run(&fixture.agent_repository, &rejected.run_id).await;
    assert_eq!(rejected_run.status, AgentRunStatus::Failed);
    assert_eq!(fixture.model_gateway.requests().await.len(), calls_before);
    assert_eq!(
        fixture
            .agent_repository
            .read_text(&rejected.run_id, &state_path)
            .await
            .unwrap()
            .text,
        source
    );
    let mut restricted = fixture
        .profile_service
        .load_profile(profile.id.as_str())
        .await
        .unwrap()
        .unwrap();
    restricted.workspace.writable_roots = vec!["output".into()];
    fixture
        .profile_service
        .save_profile(restricted, fixture.service.tool_catalog())
        .await
        .unwrap();
    select_state(&fixture, &seed_run, &base_id).await;
    let denied = fixture.service.start_run(request(&base_id)).await.unwrap();
    assert_eq!(
        wait_for_terminal_agent_run(&fixture.agent_repository, &denied.run_id)
            .await
            .status,
        AgentRunStatus::Failed
    );
    assert_eq!(fixture.model_gateway.requests().await.len(), calls_before);
    assert_eq!(
        fixture
            .agent_repository
            .read_text(&denied.run_id, &state_path)
            .await
            .unwrap()
            .text,
        source
    );
    fs::remove_dir_all(root).await.unwrap();
}

#[tokio::test]
async fn agent_state_editor_versions_edits_preserves_swipes_and_rebases_revision() {
    use tt_application::dto::agent_dto::{
        AgentCancelRunDto, AgentOutputRevisionDto, AgentResumeRunDto, AgentSaveStateDto,
        AgentStateFieldDto, AgentStateTargetDto,
    };
    use tt_ports::repositories::chat_payload_commit_repository::{
        ChatPayloadCommitRepository, ChatPayloadTarget,
    };
    let root = temp_root("state-editor");
    let source = r#"{"schemaVersion":1,"revision":1,"scene":{"location":"old"},"balance":123456789012345678901,"fraction":0.123456789012345678901}"#;
    let fixture = agent_runtime_fixture_with_responses(
        &root,
        vec![
            model_tool_response(vec![
                model_tool_call(
                    "state",
                    "workspace_write_file",
                    json!({"path":"persist/state.json","content":source}),
                ),
                model_tool_call(
                    "archive",
                    "workspace_write_file",
                    json!({"path":"persist/archive.json","content":"{\"keep\":true}"}),
                ),
                model_tool_call("finish", "workspace_finish", json!({})),
            ]),
            model_tool_response(vec![
                model_tool_call(
                    "read_manual",
                    "workspace_read_file",
                    json!({"path":"persist/state.json"}),
                ),
                model_tool_call("finish_revision", "workspace_finish", json!({})),
            ]),
        ],
    );
    let profile = resolve_contract_profile(&fixture).await;
    let handle = start_contract_agent_run(
        &fixture,
        &profile,
        AgentRunPresentation::Background,
        "state-editor",
        Some(false),
    )
    .await;
    let checkpoint = super::resume::wait_for_checkpoint(&fixture, &handle.run_id).await;
    let run = fixture
        .agent_repository
        .load_run(&handle.run_id)
        .await
        .unwrap();
    let events = read_agent_events(&fixture.agent_repository, &run.id).await;
    let state_id = events
        .iter()
        .find(|event| event.event_type == "persistent_changes_committed")
        .unwrap()
        .payload["stateId"]
        .as_str()
        .unwrap()
        .to_owned();
    let chat_target = ChatPayloadTarget::Character {
        character_id: "Alice".into(),
        file_name: "state-editor.jsonl".into(),
    };
    let metadata =
        json!({"runId":run.id,"persistStateStatus":"committed","persistStateId":state_id});
    let header = json!({"user_name":"User","character_name":"Alice","chat_metadata":{"integrity":run.stable_chat_id,"unrelated":{"keep":true}}});
    let message = json!({"name":"Alice","is_user":false,"is_system":false,"mes":"Unchanged body","swipe_id":0,
        "swipes":["Unchanged body","Other body"],"extra":{"custom":{"keep":true},"tauritavern":{"agent":metadata}},
        "swipe_info":[{"extra":{"custom":{"keep":true},"tauritavern":{"agent":metadata}}},{"extra":{"tauritavern":{"agent":metadata}}}]});
    let payload = format!("{header}\n{message}\n");
    let session = fixture
        .chat_repository
        .begin(chat_target.clone(), true, None)
        .await
        .unwrap();
    fixture
        .chat_repository
        .append(&session.session_id, 0, payload.as_bytes())
        .await
        .unwrap();
    fixture
        .chat_repository
        .finish(&session.session_id, payload.len() as u64)
        .await
        .unwrap();
    let target = AgentStateTargetDto {
        chat_ref: run.chat_ref.clone(),
        stable_chat_id: run.stable_chat_id.clone(),
        message_index: 0,
        swipe_id: Some(0),
    };
    let document = fixture
        .service
        .read_state_document(target.clone())
        .await
        .unwrap();
    assert!(!document.read_only);
    assert!(document.fields.iter().any(|field| field.pointer == "/balance" && field.value_json == "123456789012345678901"));
    let request = |text: &str| -> AgentSaveStateDto {
        serde_json::from_value(json!({"editId":uuid::Uuid::new_v4().to_string(),"target":target,"expectedStateId":document.state_id,"expectedSha256":document.sha256,"expectedChatSha256":document.chat_sha256,"text":text})).unwrap()
    };
    assert!(
        fixture
            .service
            .save_state_document(request("{invalid"))
            .await
            .is_err()
    );
    let mut form = request(source);
    form.text = None;
    form.edits = Some(vec![AgentStateFieldDto {
        pointer: "/scene/location".into(),
        kind: "string".into(),
        value_json: "\"manual\"".into(),
    }]);
    let (first, second) = tokio::join!(
        fixture
            .service
            .save_state_document(request(&source.replace("old", "manual"))),
        fixture.service.save_state_document(form),
    );
    assert_ne!(first.is_ok(), second.is_ok());
    let saved = first.or(second).unwrap();
    let mut replay = request(&source.replace("old", "manual"));
    replay.edit_id = saved.agent_metadata["manualStateEdit"]["editId"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        fixture
            .service
            .save_state_document(replay)
            .await
            .unwrap()
            .state_id,
        saved.state_id
    );
    assert_ne!(saved.state_id, state_id);
    assert!(
        fixture
            .service
            .save_state_document(request(&source.replace("old", "stale")))
            .await
            .is_err()
    );
    let original = fixture
        .agent_repository
        .read_persistent_file(
            &run.workspace_id,
            &state_id,
            &WorkspacePath::parse("persist/state.json").unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(original.text, source);
    let edited = fixture
        .service
        .read_state_document(target.clone())
        .await
        .unwrap();
    let value: Value = serde_json::from_str(&edited.text).unwrap();
    assert_eq!(value["scene"]["location"], "manual");
    assert_eq!(value["revision"], 2);
    assert_eq!(value["balance"].to_string(), "123456789012345678901");
    assert_eq!(value["fraction"].to_string(), "0.123456789012345678901");
    assert_eq!(
        fixture
            .agent_repository
            .read_persistent_file(
                &run.workspace_id,
                &saved.state_id,
                &WorkspacePath::parse("persist/archive.json").unwrap()
            )
            .await
            .unwrap()
            .text,
        "{\"keep\":true}"
    );
    assert!(
        fixture
            .chat_repository
            .replace_message_guarded(
                chat_target.clone(),
                0,
                &document.chat_sha256,
                message.clone()
            )
            .await
            .is_err()
    );
    let snapshot = fixture
        .chat_repository
        .read_message_snapshot(chat_target, 0)
        .await
        .unwrap();
    assert_eq!(snapshot.chat_metadata, header["chat_metadata"]);
    assert_eq!(snapshot.message["mes"], message["mes"]);
    assert_eq!(snapshot.message["swipes"], message["swipes"]);
    assert_eq!(
        snapshot.message["extra"]["custom"],
        message["extra"]["custom"]
    );
    assert_eq!(snapshot.message["swipe_info"][1], message["swipe_info"][1]);
    assert_eq!(
        snapshot.message["swipe_info"][0]["extra"],
        snapshot.message["extra"]
    );
    assert_eq!(fixture.model_gateway.requests().await.len(), 1);
    fixture
        .service
        .resume_run(AgentResumeRunDto {
            run_id: run.id.clone(),
            expected_terminal_seq: checkpoint.terminal_seq,
            chat_ref: run.chat_ref.clone(),
            stable_chat_id: run.stable_chat_id.clone(),
            additional_rounds: 0,
            host_presentation: false,
            revision: Some(AgentOutputRevisionDto {
                guidance: "Revise prose only".into(),
                previous_output: "Unchanged body".into(),
            }),
        })
        .await
        .unwrap();
    let revised = super::resume::wait_for_checkpoint(&fixture, &run.id).await;
    assert_eq!(revised.run.status, AgentRunStatus::Completed);
    let state = read_workspace_json(&fixture.agent_repository, &run.id, "persist/state.json").await;
    assert_eq!(state, value);
    fixture
        .model_gateway
        .wait_for_cancel_on_request
        .store(3, Ordering::SeqCst);
    let next = fixture.service.start_run(serde_json::from_value(json!({
        "chatRef":run.chat_ref,"stableChatId":run.stable_chat_id,"profileId":profile.id.as_str(),
        "promptSnapshot":{"contextPolicy":profile.context,"chatCompletionPayload":chat_request("continue after edit").payload},
        "options":{"presentation":"background"}
    })).unwrap()).await.unwrap();
    let mut requests = fixture.model_gateway.request_count.subscribe();
    tokio::time::timeout(
        AGENT_CONTRACT_ASYNC_TIMEOUT,
        requests.wait_for(|count| *count >= 3),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(
        fixture
            .agent_repository
            .load_run(&next.run_id)
            .await
            .unwrap()
            .persist_base_state_id
            .as_deref(),
        Some(saved.state_id.as_str())
    );
    assert_eq!(
        read_workspace_json(
            &fixture.agent_repository,
            &next.run_id,
            "persist/state.json"
        )
        .await,
        value
    );
    assert!(
        fixture
            .service
            .read_state_document(target.clone())
            .await
            .unwrap()
            .read_only
    );
    assert!(
        fixture
            .service
            .save_state_document(request(&source.replace("old", "blocked")))
            .await
            .unwrap_err()
            .to_string()
            .contains("read-only")
    );
    fixture
        .service
        .cancel_run(AgentCancelRunDto {
            run_id: next.run_id.clone(),
        })
        .await
        .unwrap();
    super::resume::wait_for_checkpoint(&fixture, &next.run_id).await;
    fs::remove_dir_all(root).await.unwrap();
}

#[tokio::test]
async fn reply_publication_projection_is_independent_of_event_pages_and_invocation_filters() {
    use tt_application::dto::agent_dto::AgentReadEventsDto;
    let root = temp_root("reply-publication-projection");
    let fixture = agent_runtime_fixture(&root);
    let profile = resolve_contract_profile(&fixture).await;
    let run = contract_run(
        "reply-publication",
        AgentRunPresentation::Foreground,
        &profile,
    );
    fixture.agent_repository.create_run(&run).await.unwrap();
    let read = || {
        fixture.service.read_events(AgentReadEventsDto {
            run_id: run.id.clone(),
            after_seq: None,
            before_seq: Some(u64::MAX),
            limit: 1,
            invocation_id: Some("unrelated-invocation".into()),
            include_timeline_projection: true,
        })
    };
    let empty = read()
        .await
        .unwrap()
        .timeline_projection
        .unwrap()
        .reply_publication;
    assert_eq!(empty.seq, 0);
    assert!(!empty.pending);
    for (event_type, pending) in [
        ("chat_commit_completed", true),
        ("persistent_state_metadata_updated", false),
        ("chat_commit_completed", true),
    ] {
        let publication = fixture
            .agent_repository
            .append_event(
                &run.id,
                AgentRunEventLevel::Info,
                event_type,
                json!({"invocationId":"inv_root"}),
            )
            .await
            .unwrap();
        for _ in 0..250 {
            fixture
                .agent_repository
                .append_event(
                    &run.id,
                    AgentRunEventLevel::Info,
                    "tool_call_completed",
                    json!({"invocationId":"inv_root"}),
                )
                .await
                .unwrap();
        }
        let result = read().await.unwrap();
        assert!(result.events.is_empty());
        let status = result.timeline_projection.unwrap().reply_publication;
        assert_eq!(status.seq, publication.seq);
        assert_eq!(status.pending, pending);
    }
    fs::remove_dir_all(root).await.unwrap();
}

#[tokio::test]
async fn missing_persist_requires_an_explicit_empty_start_before_creating_a_run() {
    for (label, metadata, expected_error) in [
        (
            "pending-state",
            json!({ "persistStateStatus": "not_committed", "persistStateId": "old_state" }),
            "agent.persist_state_pending",
        ),
        (
            "missing-id",
            json!({ "persistStateStatus": "committed" }),
            "agent.persist_state_missing",
        ),
        (
            "missing-version",
            json!({ "persistStateId": "state_missing" }),
            "agent.persistent_state_not_found",
        ),
        (
            "invalid-id",
            json!({ "persistStateId": "../invalid" }),
            "Invalid agent storage segment persist_state_id",
        ),
    ] {
        let root = temp_root(label);
        let fixture = agent_runtime_fixture(&root);
        let profile = resolve_contract_profile(&fixture).await;
        let mut chat = Chat::new("User", "Alice");
        chat.file_name = Some("story.jsonl".to_string());
        if label == "pending-state" {
            chat.messages.push(serde_json::from_value(json!({
                "name": "Alice", "mes": "Earlier completed reply.",
                "extra": { "tauritavern": { "agent": { "persistStateStatus": "committed", "persistStateId": "older_state" } } }
            })).unwrap());
        }
        chat.messages.push(
            serde_json::from_value(json!({
                "name": "Alice", "mes": "Keep this chat message.",
                "extra": { "tauritavern": { "agent": metadata } }
            }))
            .unwrap(),
        );
        fixture.chat_repository.save(&chat).await.unwrap();
        let mut dto: AgentStartRunDto = serde_json::from_value(json!({
            "chatRef": { "kind": "character", "characterId": "Alice", "fileName": "story.jsonl" },
            "stableChatId": "stable-story",
            "profileId": profile.id.as_str(),
            "promptSnapshot": { "contextPolicy": profile.context, "chatCompletionPayload": chat_request("continue").payload },
            "options": { "presentation": "background" }
        })).unwrap();

        let error = fixture.service.start_run(dto.clone()).await.unwrap_err();
        assert!(error.to_string().contains(expected_error), "{error}");
        assert!(
            fixture
                .agent_repository
                .list_all_runs()
                .await
                .unwrap()
                .is_empty()
        );
        assert!(fixture.model_gateway.requests().await.is_empty());

        if label != "invalid-id" {
            dto.options.start_with_empty_persist = true;
            let handle = fixture.service.start_run(dto).await.unwrap();
            // The live channel closes when runtime releases the active run; no polling sleep.
            if let Some(mut live) = fixture
                .service
                .subscribe_live_projection(&handle.run_id)
                .await
                .unwrap()
            {
                tokio::time::timeout(AGENT_CONTRACT_ASYNC_TIMEOUT, async {
                    while live.changed().await.is_ok() {}
                })
                .await
                .unwrap();
            }
            let run = fixture
                .agent_repository
                .load_run(&handle.run_id)
                .await
                .unwrap();
            assert_eq!(run.status, AgentRunStatus::Completed);
            assert_eq!(run.persist_base_state_id, None);
            assert_eq!(run.input_message_count, Some(chat.messages.len()));
            let files = fixture
                .agent_repository
                .list_files(
                    &run.id,
                    Some(&WorkspacePath::parse("persist").unwrap()),
                    1,
                    10,
                )
                .await
                .unwrap();
            assert!(files.entries.is_empty());
            assert_eq!(fixture.model_gateway.requests().await.len(), 2);
        }
        let unchanged = fixture
            .chat_repository
            .get_chat("Alice", "story.jsonl")
            .await
            .unwrap();
        assert_eq!(
            serde_json::to_value(unchanged.messages).unwrap(),
            serde_json::to_value(chat.messages).unwrap()
        );
        fs::remove_dir_all(root).await.unwrap();
    }
}
