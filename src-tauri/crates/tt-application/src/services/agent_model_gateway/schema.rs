use serde_json::{Value, json};

use crate::services::agent_model_gateway::providers::AgentProviderAdapter;
use tt_domain::models::agent::AgentModelTool;

pub(super) fn render_openai_tools(
    tools: &[AgentModelTool],
    adapter: AgentProviderAdapter,
) -> Vec<Value> {
    tools
        .iter()
        .map(|tool| {
            let native_json_schema = matches!(
                adapter,
                AgentProviderAdapter::Gemini | AgentProviderAdapter::GeminiInteractions
            ) && requires_json_schema(&tool.input_schema);
            let field = if native_json_schema && adapter == AgentProviderAdapter::Gemini {
                "parametersJsonSchema"
            } else {
                "parameters"
            };
            let parameters = if native_json_schema {
                tool.input_schema.clone()
            } else {
                sanitize_schema_for_provider(&tool.input_schema, adapter)
            };
            let mut function = json!({"name": tool.model_alias.as_str()});
            function[field] = parameters;
            if let Some(description) = &tool.description {
                function["description"] = Value::String(description.clone());
            }
            json!({
                "type": "function",
                "function": function,
            })
        })
        .collect()
}

fn requires_json_schema(schema: &Value) -> bool {
    let Some(object) = schema.as_object() else {
        return true;
    };
    if object.get("type").and_then(Value::as_str).is_none()
        || ["oneOf", "anyOf", "allOf", "$ref", "not", "if", "const"]
            .iter()
            .any(|key| object.contains_key(*key))
    {
        return true;
    }
    let properties = object.get("properties").and_then(Value::as_object);
    (object.get("type").and_then(Value::as_str) == Some("object")
        && properties.is_none_or(|properties| properties.is_empty()))
        || properties.is_some_and(|properties| properties.values().any(requires_json_schema))
        || object.get("items").is_some_and(requires_json_schema)
        || object
            .get("additionalProperties")
            .is_some_and(Value::is_object)
}

pub(super) fn sanitize_schema_for_provider(schema: &Value, adapter: AgentProviderAdapter) -> Value {
    let mut schema = schema.clone();
    remove_schema_keys(&mut schema, adapter.schema_keys_to_remove());
    if matches!(
        adapter,
        AgentProviderAdapter::Gemini | AgentProviderAdapter::GeminiInteractions
    ) {
        normalize_gemini_schema(&mut schema, 0);
    }
    schema
}

fn normalize_gemini_schema(value: &mut Value, depth: usize) {
    let Value::Object(object) = value else {
        return;
    };

    if let Some(properties) = object.get_mut("properties").and_then(Value::as_object_mut) {
        for nested in properties.values_mut() {
            normalize_gemini_schema(nested, depth + 1);
        }
    }
    if let Some(items) = object.get_mut("items") {
        normalize_gemini_schema(items, depth + 1);
    }

    if depth > 0 {
        // Gemini rejects nested required arrays in function declarations. Runtime
        // tool validators still return recoverable tool errors for missing fields.
        object.remove("required");
    } else {
        prune_required_to_declared_properties(object);
    }

    if depth > 0
        && object.get("type").and_then(Value::as_str) == Some("object")
        && object
            .get("properties")
            .and_then(Value::as_object)
            .is_none_or(|properties| properties.is_empty())
    {
        object.insert("type".to_string(), Value::String("string".to_string()));
        object.remove("properties");
        object.remove("items");
    }
}

fn prune_required_to_declared_properties(object: &mut serde_json::Map<String, Value>) {
    let Some(required) = object.get("required").and_then(Value::as_array) else {
        return;
    };
    let Some(properties) = object.get("properties").and_then(Value::as_object) else {
        object.remove("required");
        return;
    };

    let retained = required
        .iter()
        .filter_map(Value::as_str)
        .filter(|name| properties.contains_key(*name))
        .map(|name| Value::String(name.to_string()))
        .collect::<Vec<_>>();

    if retained.is_empty() {
        object.remove("required");
    } else {
        object.insert("required".to_string(), Value::Array(retained));
    }
}

fn remove_schema_keys(value: &mut Value, keys: &[&str]) {
    match value {
        Value::Object(object) => {
            for key in keys {
                object.remove(*key);
            }
            for nested in object.values_mut() {
                remove_schema_keys(nested, keys);
            }
        }
        Value::Array(items) => {
            for item in items {
                remove_schema_keys(item, keys);
            }
        }
        _ => {}
    }
}
