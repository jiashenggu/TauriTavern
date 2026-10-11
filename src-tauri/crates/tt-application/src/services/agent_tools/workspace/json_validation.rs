use serde::Serialize;
use serde_json::{Map, Value};

use super::args::optional_bool_arg;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct JsonValidationSummary {
    valid: bool,
    root_type: &'static str,
    required_keys_checked: usize,
}

pub(super) fn validate_json(
    args: &Map<String, Value>,
    text: &str,
) -> Result<Option<JsonValidationSummary>, (&'static str, String)> {
    let enabled = optional_bool_arg(args, "validate_json")
        .map_err(|message| ("tool.invalid_arguments", message))?
        .unwrap_or(false);
    if !enabled {
        return if args.contains_key("required_keys") {
            Err((
                "tool.invalid_arguments",
                "required_keys requires validate_json=true".into(),
            ))
        } else {
            Ok(None)
        };
    }
    let keys = match args.get("required_keys") {
        None => Vec::new(),
        Some(value) => {
            let items = value
                .as_array()
                .filter(|items| items.len() <= 64)
                .ok_or_else(|| {
                    (
                        "tool.invalid_arguments",
                        "required_keys must be an array of at most 64 field names".to_string(),
                    )
                })?;
            items
                .iter()
                .map(|item| {
                    item.as_str()
                        .filter(|key| !key.trim().is_empty())
                        .ok_or_else(|| {
                            (
                                "tool.invalid_arguments",
                                "required_keys entries must be nonempty strings".to_string(),
                            )
                        })
                })
                .collect::<Result<Vec<_>, _>>()?
        }
    };
    let value: Value = tt_domain::json_input::from_str(text).map_err(|error| {
        (
            "workspace.invalid_json",
            format!("JSON validation failed: {error}; no file was written"),
        )
    })?;
    if !keys.is_empty() {
        let object = value.as_object().ok_or_else(|| {
            (
                "workspace.json_required_keys",
                "required_keys requires a JSON object root; no file was written".to_string(),
            )
        })?;
        if let Some(key) = keys.iter().find(|key| !object.contains_key(**key)) {
            return Err((
                "workspace.json_required_keys",
                format!("missing required top-level field `{key}`; no file was written"),
            ));
        }
    }
    let root_type = match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    };
    Ok(Some(JsonValidationSummary {
        valid: true,
        root_type,
        required_keys_checked: keys.len(),
    }))
}
