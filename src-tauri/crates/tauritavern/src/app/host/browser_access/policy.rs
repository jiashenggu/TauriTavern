use serde_json::Value;

// Deliberately explicit: new desktop commands do not become network capabilities.
// No shell, arbitrary filesystem, native window, sync, install, log, or debug APIs.
const COMMANDS: &[&str] = &[
    "is_ready",
    "get_version",
    "get_client_version",
    "wait_for_backend_ready",
    "get_bootstrap_snapshot",
    "get_user_directory",
    "get_all_users",
    "get_user",
    "get_sillytavern_settings",
    "get_tauritavern_settings",
    "save_user_settings",
    "save_user_settings_patch",
    "get_settings_snapshots",
    "load_settings_snapshot",
    "get_all_characters",
    "get_character",
    "get_character_chats_by_id",
    "create_character",
    "update_character",
    "update_character_card_data",
    "merge_character_card_data",
    "rename_character",
    "duplicate_character",
    "delete_character",
    "get_all_chats",
    "get_chat",
    "get_character_chats",
    "create_chat",
    "add_message",
    "rename_chat",
    "delete_chat",
    "search_chats",
    "list_chat_summaries",
    "list_recent_chat_summaries",
    "chat_history_generation_started",
    "chat_history_generation_finished",
    "get_chat_payload_tail",
    "get_chat_payload_before",
    "get_chat_payload_before_pages",
    "read_character_chat_messages",
    "open_cold_chat",
    "open_cold_swipe_record",
    "read_chat_bytes",
    "plugin:resources|close",
    "begin_chat_commit",
    "append_chat_commit_chunk",
    "finish_chat_commit",
    "abort_chat_commit",
    "commit_chat_metadata",
    "get_all_groups",
    "get_group",
    "create_group",
    "update_group",
    "delete_group",
    "search_group_chats",
    "list_group_chat_summaries",
    "list_recent_group_chat_summaries",
    "get_group_chat_payload_tail",
    "get_group_chat_payload_before",
    "get_group_chat_payload_before_pages",
    "rename_group_chat",
    "delete_group_chat",
    "get_character_chat_summary",
    "get_character_chat_metadata",
    "get_group_chat_summary",
    "get_group_chat_metadata",
    "get_world_info",
    "get_world_infos_batch",
    "normalize_world_info_name",
    "save_world_info",
    "delete_world_info",
    "get_all_backgrounds",
    "get_all_background_metadata",
    "get_background_folders",
    "get_avatars",
    "get_personas",
    "list_sprites",
    "get_assets_library",
    "get_character_assets",
    "list_user_images",
    "list_user_image_folders",
    "verify_user_files",
    "get_extensions",
    "read_frontend_template",
    "read_frontend_extension_template",
    "list_presets",
    "get_preset",
    "preset_exists",
    "save_preset",
    "delete_preset",
    "restore_preset",
    "save_openai_preset",
    "delete_openai_preset",
    "save_theme",
    "delete_theme",
    "save_quick_reply_set",
    "delete_quick_reply_set",
    "read_secret_state",
    "read_secret_settings",
    "write_secret",
    "delete_secret",
    "rotate_secret",
    "rename_secret",
    "list_llm_connections",
    "load_llm_connection",
    "save_llm_connection",
    "delete_llm_connection",
    "get_chat_completions_status",
    "generate_chat_completion",
    "start_chat_completion_stream",
    "read_chat_completion_stream",
    "close_chat_completion_stream",
    "cancel_chat_completion_generation",
    "get_openrouter_model_providers",
    "get_openrouter_credits",
    "get_nanogpt_model_providers",
    "get_nanogpt_credits",
    "get_siliconflow_embedding_models",
    "get_workers_ai_embedding_models",
    "get_workers_ai_multimodal_models",
    "count_openai_tokens_batch",
    "count_openai_token_prefixes",
    "encode_tokens",
    "decode_tokens",
    "build_openai_logit_bias",
    // Read-only discovery prevents optional settings panels failing at startup.
    "get_extension_store_json",
    "try_get_extension_store_json",
    "list_agent_profiles",
    "list_agent_tools",
    "list_agent_runs",
    "list_mcp_servers",
    "list_legacy_mcp_tools",
    "list_skills",
];

pub(super) fn validate(payload: &Value) -> Result<(), &'static str> {
    let command = payload["command"]
        .as_str()
        .ok_or("Missing browser command")?;
    if !COMMANDS.contains(&command) {
        return Err("This capability is available only in the desktop app");
    }
    if command == "update_character_card_data" && payload["args"]["dto"]["avatar_path"].is_string()
    {
        return Err("Native avatar paths are available only in the desktop app");
    }
    if let Some(headers) = payload.get("headers") {
        let headers = headers.as_object().ok_or("Invalid command headers")?;
        for (name, value) in headers {
            if !["session-id", "offset", "chunk-encoding"].contains(&name.as_str())
                || !value.is_string()
            {
                return Err("Unsupported command header");
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn transport_rejects_native_authority_and_preserves_chat_commit_headers() {
        for name in [
            "plugin:fs|open",
            "set_data_root",
            "emit_event",
            "devlog_get_llm_api_log_raw",
            "install_extension",
        ] {
            assert!(validate(&json!({"command": name})).is_err());
        }
        assert!(
            validate(&json!({"command": "append_chat_commit_chunk", "headers": {
                "session-id": "test-session", "offset": "0", "chunk-encoding": "base64"
            }}))
            .is_ok()
        );
        assert!(
            validate(&json!({"command": "get_version", "headers": {
                "Tauri-Invoke-Key": "not-a-key"
            }}))
            .is_err()
        );
    }
}
