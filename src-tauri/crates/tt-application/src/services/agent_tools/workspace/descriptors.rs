use serde_json::json;

use super::{
    MODEL_WORKSPACE_ROOTS_FOR_MODEL, WORKSPACE_APPLY_PATCH, WORKSPACE_COMMIT, WORKSPACE_FINISH,
    WORKSPACE_LIST_FILES, WORKSPACE_READ_FILE, WORKSPACE_SEARCH_FILES, WORKSPACE_WRITE_FILE,
};
use tt_domain::models::tool::{ToolDescriptor, ToolId};

pub(in crate::services::agent_tools) fn workspace_list_files_descriptor() -> ToolDescriptor {
    ToolDescriptor {
        id: ToolId::builtin(WORKSPACE_LIST_FILES).expect("builtin tool name must be valid"),
        title: Some("Workspace List Files".to_string()),
        description: Some(format!(
            "List visible Agent workspace files under {MODEL_WORKSPACE_ROOTS_FOR_MODEL}. Use this before reading when you need to inspect available artifacts."
        )),
        input_schema: json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Optional relative workspace directory or file path. Omit to list the visible workspace roots."
                },
                "depth": {
                    "type": "integer",
                    "description": "Directory depth to list. Defaults to 2; maximum is 4."
                }
            }
        }),
        output_schema: None,
        annotations: json!({ "readOnly": true }),
    }
}

pub(in crate::services::agent_tools) fn workspace_read_file_descriptor() -> ToolDescriptor {
    ToolDescriptor {
        id: ToolId::builtin(WORKSPACE_READ_FILE).expect("builtin tool name must be valid"),
        title: Some("Workspace Read File".to_string()),
        description: Some("Read a visible UTF-8 Agent workspace file with line numbers. Omit start_line and line_count to read the full file; oversized files return a bounded preview with the next line to read. Reading all complete lines across pages of the same SHA establishes fullRead=true; any version change resets coverage, and truncated lines do not count as complete. Read the exact text you want to replace before using workspace_apply_patch; if a patch fails, fully read the file before retrying. `path` MUST refer to a regular file (e.g. `persist/MEMORY.md`), NOT a directory or workspace root (`persist`, `output`, ...). Call workspace_list_files first when you do not know which file to open.".to_string()),
        input_schema: json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "path": {
                    "type": "string",
                    "description": format!("Relative visible workspace file path under {MODEL_WORKSPACE_ROOTS_FOR_MODEL}, or a read-only baseline/ path explicitly provided by runtime.")
                },
                "start_line": {
                    "type": "integer",
                    "minimum": 1,
                    "description": "1-based starting line. Omit to start at line 1."
                },
                "line_count": {
                    "type": "integer",
                    "minimum": 1,
                    "description": "Number of lines to read. Omit to read through the end; oversized results return a shorter preview."
                }
            },
            "required": ["path"]
        }),
        output_schema: None,
        annotations: json!({ "readOnly": true }),
    }
}

pub(in crate::services::agent_tools) fn workspace_search_files_descriptor() -> ToolDescriptor {
    ToolDescriptor {
        id: ToolId::builtin(WORKSPACE_SEARCH_FILES).expect("builtin tool name must be valid"),
        title: Some("Workspace Search Files".to_string()),
        description: Some(format!(
            "Search visible UTF-8 Agent workspace files under {MODEL_WORKSPACE_ROOTS_FOR_MODEL}. Results return snippets and refs; use workspace_read_file to read exact ranges."
        )),
        input_schema: json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Plain text to search for in visible workspace files."
                },
                "path": {
                    "type": "string",
                    "description": format!("Optional visible workspace file or directory path under {MODEL_WORKSPACE_ROOTS_FOR_MODEL}. Omit to search all visible roots.")
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum hits to return. Defaults to 20; maximum is 50."
                },
                "context_lines": {
                    "type": "integer",
                    "description": "Context lines before and after each match. Defaults to 2; maximum is 5."
                }
            },
            "required": ["query"]
        }),
        output_schema: None,
        annotations: json!({ "readOnly": true }),
    }
}

pub(in crate::services::agent_tools) fn workspace_write_file_descriptor() -> ToolDescriptor {
    ToolDescriptor {
        id: ToolId::builtin(WORKSPACE_WRITE_FILE).expect("builtin tool name must be valid"),
        title: Some("Workspace Write File".to_string()),
        description: Some("Write UTF-8 text to a writable Agent workspace file. Provide exactly one of content or source_path. source_path copies a visible file already fully read (or created) by this invocation without regenerating its text; stale sources are rejected. Existing targets still require a prior read and matching SHA. create_only atomically rejects any existing target, even one already read. source_path optionally accepts either text edits or json_ops to correct the copied data before one target write, leaving the source unchanged; both require source_path and cannot be mixed. Each batch is limited to 128 replacements and cannot grow beyond 8 MiB or the original size. source_path and create_only require mode replace. In a foreground invocation, copying the configured message body can display it after this model turn, before persistent state is finalized, until the first explicit commit. Adopt reviewed prose in its own turn if it should be visible before generating state edits. This progress display does not finish the run. mode append adds content exactly to the end; include a leading newline when needed.".to_string()),
        input_schema: json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "path": {
                    "type": "string",
                    "description": format!("Relative workspace path. Writable prefixes are {MODEL_WORKSPACE_ROOTS_FOR_MODEL}.")
                },
                "content": {
                    "type": "string",
                    "description": "Complete UTF-8 file content for replace, or the exact suffix to add for append. Mutually exclusive with source_path."
                },
                "source_path": {
                    "type": "string",
                    "description": "Copy the exact fully-read source version from this workspace. Mutually exclusive with content; source must be visible, unchanged and different from path."
                },
                "edits": patch_edits_schema(),
                "json_ops": json_ops_schema(),
                "validate_json": {
                    "type": "boolean",
                    "description": "Before writing, parse the complete result as JSON. Invalid JSON leaves the target unchanged. Replace mode only; defaults to false."
                },
                "required_keys": json_required_keys_schema(),
                "finalize": finalize_schema(),
                "finish_task": {
                    "type": "boolean",
                    "description": "Return-mode child only: after this write succeeds, return this file with its SHA and finish the task without another model call. Requires task.return permission and must be the last call in the turn. Defaults to false."
                },
                "create_only": {
                    "type": "boolean",
                    "description": "Require a new target, without overwriting an existing file. Defaults to false; use true for immutable baseline copies. Only supported in replace mode."
                },
                "mode": {
                    "type": "string",
                    "enum": ["replace", "append"],
                    "description": "replace writes the complete file; append adds content to the end, creating the file if missing. Defaults to replace."
                }
            },
            "required": ["path"]
        }),
        output_schema: None,
        annotations: json!({ "mutating": true }),
    }
}

pub(in crate::services::agent_tools) fn workspace_apply_patch_descriptor() -> ToolDescriptor {
    ToolDescriptor {
        id: ToolId::builtin(WORKSPACE_APPLY_PATCH).expect("builtin tool name must be valid"),
        title: Some("Workspace Apply Patch".to_string()),
        description: Some("Update one file with text edits OR json_ops, never both. json_ops applies add/replace/remove/test at JSON Pointer paths (escape ~ as ~0 and / as ~1; add at /array/- appends). add requires an existing parent, replace/remove require an existing target, test checks JSON value equality. Root removal is forbidden. JSON operations require a full read, validate the whole resulting JSON and required_keys, serialize it as indented JSON, and write once with the observed SHA; any failed test/path/validation leaves the file unchanged. Use only new values rather than repeating old text. Use edits for text replacements: 1-128 replacements applied in order to a fully-read file, then written once with its observed SHA. Later edits may reference earlier results. Any invalid edit, match failure or conflict leaves the file unchanged. Batch output cannot grow beyond 8 MiB or the original file size, whichever is larger. Alternatively supply legacy top-level old_string, new_string and optional replace_all; do not mix these with edits. Each old_string must match exactly and uniquely unless replace_all is true. After failure, read the file again before retrying.".to_string()),
        input_schema: json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "path": {
                    "type": "string",
                    "description": format!("Relative writable workspace file path under {MODEL_WORKSPACE_ROOTS_FOR_MODEL}.")
                },
                "old_string": {
                    "type": "string",
                    "description": "Exact text to replace. Do not include line number prefixes from read output."
                },
                "new_string": {
                    "type": "string",
                    "description": "Replacement text."
                },
                "replace_all": {
                    "type": "boolean",
                    "description": "Replace every occurrence of old_string. Defaults to false."
                },
                "edits": patch_edits_schema(),
                "json_ops": json_ops_schema(),
                "validate_json": {
                    "type": "boolean",
                    "description": "Parse the full patched result as JSON before the guarded write. Invalid JSON leaves the file unchanged; defaults to false."
                },
                "required_keys": json_required_keys_schema(),
                "finalize": finalize_schema()
            },
            "required": ["path"]
        }),
        output_schema: None,
        annotations: json!({ "mutating": true }),
    }
}

fn finalize_schema() -> serde_json::Value {
    json!({"type":"boolean","description":"Optional conditional finalization after this persist/ mutation succeeds. Must be the ONLY tool call in this model turn; all other file work must already be confirmed. Requires commit/finish capabilities, no pending child tasks and a fully-read/adopted body matching the path and SHA of the latest confirmed replace commit. Append fragments and unpublished rewrites must not be finalized; preserve append output by finishing separately, or first explicitly replace with the complete reviewed body. Validation or write failure never commits or finishes. Success commits the reviewed body then publishes state through normal host acknowledgments. If publication fails after the file was saved, retry publication, not the mutation. Defaults to false; incompatible with finish_task."})
}

fn json_ops_schema() -> serde_json::Value {
    json!({
        "type":"array","minItems":1,"maxItems":128,
        "items":{"type":"object","additionalProperties":false,
            "properties":{"op":{"type":"string","enum":["add","replace","remove","test"]},"path":{"type":"string"},"value":{}},
            "required":["op","path"]
        },
        "description":"Ordered JSON Pointer operations; add/replace/test require value, remove must omit it. Implies JSON validation. On write_file requires source_path; cannot be mixed with text edits."
    })
}

fn json_required_keys_schema() -> serde_json::Value {
    json!({
        "type": "array", "maxItems": 64,
        "items": { "type": "string", "minLength": 1 },
        "description": "Optional required top-level field names. Requires validate_json=true or json_ops; a nonempty list also requires an object root. Checks existence, not types, arithmetic or narrative facts."
    })
}

fn patch_edits_schema() -> serde_json::Value {
    json!({
        "type": "array",
        "minItems": 1,
        "maxItems": 128,
        "description": "Ordered exact replacements applied in memory before one guarded write. Requires a full read of the source text; all edits must succeed. For write_file, requires source_path. For apply_patch, replaces the top-level old_string/new_string/replace_all arguments.",
        "items": {
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "old_string": { "type": "string", "minLength": 1 },
                "new_string": { "type": "string" },
                "replace_all": { "type": "boolean" }
            },
            "required": ["old_string", "new_string"]
        }
    })
}

pub(in crate::services::agent_tools) fn workspace_finish_descriptor() -> ToolDescriptor {
    ToolDescriptor {
        id: ToolId::builtin(WORKSPACE_FINISH).expect("builtin tool name must be valid"),
        title: Some("Workspace Finish".to_string()),
        description: Some("Finish the Agent run after required foreground chat commits and workspace changes are complete.".to_string()),
        input_schema: json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "reason": {
                    "type": "string",
                    "description": "Short completion reason."
                }
            }
        }),
        output_schema: None,
        annotations: json!({ "control": true }),
    }
}

pub(in crate::services::agent_tools) fn workspace_commit_descriptor() -> ToolDescriptor {
    ToolDescriptor {
        id: ToolId::builtin(WORKSPACE_COMMIT).expect("builtin tool name must be valid"),
        title: Some("Workspace Commit".to_string()),
        description: Some("Commit a workspace text file to the current chat message. With no arguments, replace the current run message with output/main.md. append adds the file text to the same message, creating it when this run has not committed yet. After all reviewed files and state changes are prepared successfully, set finish=true to end and publish the persistent version automatically after host commit confirmation. A rejected commit does not end the run. This requires finish permission and must be the last tool call in the turn. Otherwise keep editing and finish separately with workspace_finish. Do not reply in plain text as the final answer.".to_string()),
        input_schema: json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Relative visible workspace file path to publish. Defaults to output/main.md."
                },
                "mode": {
                    "type": "string",
                    "enum": ["replace", "append"],
                    "description": "replace overwrites this run's chat message; append appends to the same message. Defaults to replace."
                },
                "reason": {
                    "type": "string",
                    "description": "Short commit reason."
                },
                "finish": {
                    "type": "boolean",
                    "description": "End the run only after this chat commit is confirmed, then use normal persistent-state publication and recovery. Prepare and verify all intended workspace changes first. Defaults to false."
                }
            }
        }),
        output_schema: None,
        annotations: json!({ "control": true, "mutating": true }),
    }
}
