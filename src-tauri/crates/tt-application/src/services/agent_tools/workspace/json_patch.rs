use serde_json::{Map, Value};

type PatchError = (&'static str, String);

enum Operation {
    Add { path: String, value: Value },
    Replace { path: String, value: Value },
    Remove { path: String },
    Test { path: String, value: Value },
}

impl Operation {
    fn parse(raw: &Value) -> Result<Self, PatchError> {
        let invalid = || {
            failure(
                "invalid operation shape; use add, replace, remove or test with path and value as required",
            )
        };
        let object = raw.as_object().ok_or_else(invalid)?;
        if object
            .keys()
            .any(|key| !matches!(key.as_str(), "op" | "path" | "value"))
        {
            return Err(invalid());
        }
        let path = object
            .get("path")
            .and_then(Value::as_str)
            .ok_or_else(invalid)?
            .to_owned();
        match object.get("op").and_then(Value::as_str) {
            Some("remove") if !object.contains_key("value") => Ok(Self::Remove { path }),
            Some(op @ ("add" | "replace" | "test")) => {
                let value = object.get("value").ok_or_else(invalid)?.clone();
                Ok(match op {
                    "add" => Self::Add { path, value },
                    "replace" => Self::Replace { path, value },
                    _ => Self::Test { path, value },
                })
            }
            _ => Err(invalid()),
        }
    }
}

fn json_values_equal(left: &Value, right: &Value) -> bool {
    match (left, right) {
        (Value::Number(left), Value::Number(right)) => {
            if left == right {
                return true;
            }
            let left = left.to_string();
            let right = right.to_string();
            let (left_negative, left_digits, left_exponent, left_shift) = decimal_parts(&left);
            let (right_negative, right_digits, right_exponent, right_shift) = decimal_parts(&right);
            left_digits == right_digits
                && (left_digits.is_empty()
                    || (left_negative == right_negative
                        && decimal_exponents_equal(
                            left_exponent,
                            right_exponent,
                            left_shift - right_shift,
                        )))
        }
        (Value::Array(left), Value::Array(right)) => {
            left.len() == right.len()
                && left
                    .iter()
                    .zip(right)
                    .all(|(left, right)| json_values_equal(left, right))
        }
        (Value::Object(left), Value::Object(right)) => {
            left.len() == right.len()
                && left.iter().all(|(key, left)| {
                    right
                        .get(key)
                        .is_some_and(|right| json_values_equal(left, right))
                })
        }
        _ => left == right,
    }
}

fn decimal_parts(text: &str) -> (bool, String, &str, i128) {
    let negative = text.starts_with('-');
    let unsigned = text.trim_start_matches('-');
    let (mantissa, exponent) = unsigned.split_once(['e', 'E']).unwrap_or((unsigned, "0"));
    let fraction_digits = mantissa
        .split_once('.')
        .map_or(0, |(_, fraction)| fraction.len());
    let digits = mantissa.replace('.', "");
    let digits = digits.trim_start_matches('0');
    let significant = digits.trim_end_matches('0');
    let shift = (digits.len() - significant.len()) as i128 - fraction_digits as i128;
    (negative, significant.to_owned(), exponent, shift)
}

fn decimal_exponents_equal(left: &str, right: &str, mut difference: i128) -> bool {
    let left_sign = if left.starts_with('-') { -1 } else { 1 };
    let right_sign = if right.starts_with('-') { -1 } else { 1 };
    let mut left = left.trim_start_matches(['-', '+']).bytes().rev();
    let mut right = right.trim_start_matches(['-', '+']).bytes().rev();
    loop {
        let digits = (left.next(), right.next());
        if digits == (None, None) {
            return difference == 0;
        }
        difference += left_sign * digits.0.map_or(0, |digit| i128::from(digit - b'0'))
            - right_sign * digits.1.map_or(0, |digit| i128::from(digit - b'0'));
        if difference % 10 != 0 {
            return false;
        }
        difference /= 10;
    }
}

fn failure(message: impl Into<String>) -> PatchError {
    ("workspace.json_patch_invalid", message.into())
}

fn decode(segment: &str) -> Result<String, PatchError> {
    let mut output = String::new();
    let mut chars = segment.chars();
    while let Some(ch) = chars.next() {
        if ch != '~' {
            output.push(ch);
            continue;
        }
        output.push(match chars.next() {
            Some('0') => '~',
            Some('1') => '/',
            _ => return Err(failure("invalid JSON Pointer escape")),
        });
    }
    Ok(output)
}

fn index(key: &str, len: usize, insertion: bool) -> Result<usize, PatchError> {
    if key == "-" && insertion {
        return Ok(len);
    }
    if key.is_empty()
        || (key.starts_with('0') && key.len() > 1)
        || !key.bytes().all(|c| c.is_ascii_digit())
    {
        return Err(failure("invalid array index"));
    }
    key.parse::<usize>()
        .ok()
        .filter(|i| *i < len || (insertion && *i == len))
        .ok_or_else(|| failure("array index out of bounds"))
}

pub(super) fn apply_json_operations(
    text: &str,
    args: &Map<String, Value>,
) -> Result<(String, usize, usize), PatchError> {
    if ["edits", "old_string", "new_string", "replace_all"]
        .iter()
        .any(|key| args.contains_key(*key))
    {
        return Err(failure("json_ops cannot be mixed with text edits"));
    }
    let raw = args
        .get("json_ops")
        .and_then(Value::as_array)
        .filter(|ops| !ops.is_empty() && ops.len() <= 128)
        .ok_or_else(|| failure("json_ops must contain1–128 operations"))?;
    if serde_json::to_vec(raw)
        .map_err(|_| failure("operation serialization failed"))?
        .len()
        > 8 * 1024 * 1024
    {
        return Err(failure("JSON operation data exceeds8 MiB"));
    }
    let operations = raw
        .iter()
        .map(Operation::parse)
        .collect::<Result<Vec<_>, _>>()?;
    let mut document: Value = tt_domain::json_input::from_str(text).map_err(|error| {
        failure(format!(
            "source JSON rejected: {error}; no edits were written"
        ))
    })?;
    let original = document.clone();
    let mut changes = 0;
    for (number, operation) in operations.iter().enumerate() {
        let path = match operation {
            Operation::Add { path, .. }
            | Operation::Replace { path, .. }
            | Operation::Remove { path }
            | Operation::Test { path, .. } => path,
        };
        let apply = |document: &mut Value| -> Result<bool, PatchError> {
            if path.len() > 4096 || (!path.is_empty() && !path.starts_with('/')) {
                return Err(failure("invalid JSON Pointer path"));
            }
            for segment in path.split('/').skip(1) {
                decode(segment)?;
            }
            if let Operation::Test { value, .. } = operation {
                if !document
                    .pointer(path)
                    .is_some_and(|actual| json_values_equal(actual, value))
                {
                    return Err((
                        "workspace.json_patch_test_failed",
                        "JSON value test failed".into(),
                    ));
                }
                return Ok(false);
            }
            if path.is_empty() {
                match operation {
                    Operation::Add { value, .. } | Operation::Replace { value, .. } => {
                        *document = value.clone()
                    }
                    _ => return Err(failure("cannot remove the document root")),
                }
                return Ok(true);
            }
            let (parent, key) = path.rsplit_once('/').expect("nonempty pointer");
            let key = decode(key)?;
            let target = document
                .pointer_mut(parent)
                .ok_or_else(|| failure("parent path does not exist"))?;
            match target {
                Value::Object(map) => match operation {
                    Operation::Add { value, .. } => {
                        map.insert(key, value.clone());
                    }
                    Operation::Replace { value, .. } => {
                        *map.get_mut(&key)
                            .ok_or_else(|| failure("replace path does not exist"))? = value.clone();
                    }
                    Operation::Remove { .. } => {
                        map.remove(&key)
                            .ok_or_else(|| failure("remove path does not exist"))?;
                    }
                    Operation::Test { .. } => unreachable!(),
                },
                Value::Array(items) => match operation {
                    Operation::Add { value, .. } => {
                        let i = index(&key, items.len(), true)?;
                        items.insert(i, value.clone());
                    }
                    Operation::Replace { value, .. } => {
                        let i = index(&key, items.len(), false)?;
                        items[i] = value.clone();
                    }
                    Operation::Remove { .. } => {
                        let i = index(&key, items.len(), false)?;
                        items.remove(i);
                    }
                    Operation::Test { .. } => unreachable!(),
                },
                _ => return Err(failure("parent path must be an object or array")),
            }
            Ok(true)
        };
        changes += usize::from(apply(&mut document).map_err(|(code, message)| {
            (
                code,
                format!("operation{}: {message}; no edits were written", number + 1),
            )
        })?);
    }
    if document == original {
        return Err((
            "workspace.patch_no_change",
            "JSON operations leave the document unchanged".into(),
        ));
    }
    let updated = serde_json::to_string_pretty(&document)
        .map_err(|_| failure("JSON serialization failed"))?
        + "\n";
    if updated.len() > (8 * 1024 * 1024).max(text.len()) {
        return Err(failure(
            "JSON result exceeds size limit; no edits were written",
        ));
    }
    Ok((updated, changes, operations.len()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn json_tests_compare_values_without_numeric_rounding_or_representation_bias() {
        for (actual, expected, equal) in [
            ("1.0", "1", true),
            ("100.00", "1e2", true),
            ("-0.012300", "-123e-4", true),
            ("0", "-0.0e99999999999999999999999999999999999999999", true),
            ("18446744073709551617", "18446744073709551617.0", true),
            ("0.123456789012345678901", "123456789012345678901e-21", true),
            (
                "10e99999999999999999999999999999999999999999",
                "1e100000000000000000000000000000000000000000",
                true,
            ),
            (
                "1e-99999999999999999999999999999999999999999",
                "10e-100000000000000000000000000000000000000000",
                true,
            ),
            ("0.001e+0003", "1e-0000", true),
            (
                r#"{"a":[1.0,{"n":-0.0}],"b":true}"#,
                r#"{"b":true,"a":[1,{"n":0}]}"#,
                true,
            ),
            ("18446744073709551617", "18446744073709551618", false),
            ("0.123456789012345678901", "0.123456789012345678902", false),
            ("1e-99999999999999999999999999999999999999999", "0", false),
            (
                "1e99999999999999999999999999999999999999999",
                "1e100000000000000000000000000000000000000000",
                false,
            ),
            ("-1", "1.0", false),
            ("1e-2", "1e2", false),
            ("1", "true", false),
            ("1", r#""1""#, false),
            (r#"{"a":null}"#, "{}", false),
            ("[1,2]", "[2.0,1.0]", false),
            ("[1]", "[1.0,1]", false),
            (
                r#"{"a":[1.0]}"#,
                r#"{"a":[1.000000000000000000001]}"#,
                false,
            ),
        ] {
            let source = format!(r#"{{"value":{actual},"revision":0}}"#);
            let expected: Value = serde_json::from_str(expected).unwrap();
            let args = json!({"json_ops":[
                {"op":"replace","path":"/revision","value":1},
                {"op":"test","path":"/value","value":expected}
            ]});
            let result = apply_json_operations(&source, args.as_object().unwrap());
            if equal {
                let (text, changes, count) = result
                    .unwrap_or_else(|error| panic!("{actual} should equal {expected}: {error:?}"));
                let updated: Value = serde_json::from_str(&text).unwrap();
                let original: Value = serde_json::from_str(&source).unwrap();
                assert_eq!(updated["value"], original["value"]);
                assert_eq!(updated["revision"], 1);
                assert_eq!((changes, count), (1, 2));
            } else {
                let error = result.unwrap_err();
                assert_eq!(
                    error.0, "workspace.json_patch_test_failed",
                    "{actual} vs {expected}: {error:?}"
                );
            }
        }
        let args = json!({"json_ops":[
            {"op":"add","path":"/revision","value":1},
            {"op":"test","path":"/missing","value":null}
        ]});
        assert_eq!(
            apply_json_operations("{}", args.as_object().unwrap())
                .unwrap_err()
                .0,
            "workspace.json_patch_test_failed"
        );
    }

    #[test]
    fn json_patches_preserve_untouched_and_supplied_number_precision() {
        let source =
            r#"{"id":18446744073709551617,"balance":0.123456789012345678901,"revision":1}"#;
        let args: Value = serde_json::from_str(r#"{"json_ops":[{"op":"replace","path":"/revision","value":2},{"op":"add","path":"/amount","value":0.987654321098765432109},{"op":"add","path":"/largeInteger","value":18446744073709551618},{"op":"add","path":"/nested","value":{"items":[-18446744073709551618]}}]}"#).unwrap();
        let (text, _, _) = apply_json_operations(source, args.as_object().unwrap()).unwrap();
        let result: Value = serde_json::from_str(&text).unwrap();
        for (key, expected) in [
            ("id", "18446744073709551617"),
            ("balance", "0.123456789012345678901"),
            ("amount", "0.987654321098765432109"),
            ("largeInteger", "18446744073709551618"),
        ] {
            assert_eq!(result[key].as_number().unwrap().to_string(), expected);
        }
        assert_eq!(result["revision"], 2);
        assert_eq!(
            result["nested"]["items"][0].to_string(),
            "-18446744073709551618"
        );
    }

    #[test]
    fn json_operations_reject_malformed_shapes_without_a_partial_result() {
        for operation in [
            json!({"op":"replace","path":"/value"}),
            json!({"op":"add","path":"/value"}),
            json!({"op":"test","path":"/value"}),
            json!({"op":"remove","path":"/value","value":null}),
            json!({"op":"replace","path":"/value","value":1,"extra":true}),
            json!({"op":"replace","path":1,"value":1}),
            json!({"op":"copy","path":"/value","from":"/other"}),
            json!({"path":"/value","value":1}),
            Value::Null,
        ] {
            let args = json!({"json_ops":[{"op":"add","path":"/revision","value":1},operation]});
            assert_eq!(
                apply_json_operations(r#"{"value":0}"#, args.as_object().unwrap())
                    .unwrap_err()
                    .0,
                "workspace.json_patch_invalid"
            );
        }
    }

    #[test]
    fn json_paths_preserve_types_escape_keys_and_order_array_mutations() {
        let original = json!({"a/b":{"~key":[1,2]},"旧项":true});
        let args = json!({"json_ops":[
            {"op":"test","path":"/a~1b/~0key/1","value":2},
            {"op":"add","path":"/a~1b/~0key/-","value":{"name":"new","active":false}},
            {"op":"remove","path":"/a~1b/~0key/0"},
            {"op":"replace","path":"/a~1b/~0key/0","value":null},
            {"op":"remove","path":"/旧项"}
        ]});
        let (text, changes, count) =
            apply_json_operations(&original.to_string(), args.as_object().unwrap()).unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&text).unwrap(),
            json!({"a/b":{"~key":[null,{"name":"new","active":false}]}})
        );
        assert_eq!((changes, count), (4, 5));
        for path in ["/a~2b", "/a~1b/~0key/01", "/a~1b/~0key/-", "/absent/child"] {
            let args = json!({"json_ops":[{"op":"replace","path":path,"value":0}]});
            assert!(
                apply_json_operations(&original.to_string(), args.as_object().unwrap()).is_err()
            );
        }
    }
}
