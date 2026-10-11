use std::fmt;

use serde::de::{Error, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer};
use serde_json::value::RawValue;
use serde_json::{Result, Value};

const RESERVED_NUMBER_KEY: &str = "$serde_json::private::Number";
const RESERVED_RAW_KEY: &str = "$serde_json::private::RawValue";
const MAX_DEPTH: usize = 128;

pub fn from_str<'a, T: Deserialize<'a>>(text: &'a str) -> Result<T> {
    validate_raw(serde_json::from_str(text)?, 0)?;
    serde_json::from_str(text)
}

pub fn from_slice<'a, T: Deserialize<'a>>(bytes: &'a [u8]) -> Result<T> {
    validate_slice(bytes)?;
    serde_json::from_slice(bytes)
}

pub fn validate_slice(bytes: &[u8]) -> Result<()> {
    validate_raw(serde_json::from_slice(bytes)?, 0)
}

pub fn validate_value(value: &Value) -> Result<()> {
    validate_value_at_depth(value, 0)
}

fn check_key(key: &str) -> Result<()> {
    if key == RESERVED_NUMBER_KEY || key == RESERVED_RAW_KEY {
        return Err(serde_json::Error::custom(format!(
            "json.reserved_member: literal object member `{key}` collides with a private Serde JSON representation"
        )));
    }
    Ok(())
}

fn check_depth(depth: usize) -> Result<()> {
    if depth >= MAX_DEPTH {
        return Err(serde_json::Error::custom(
            "JSON nesting exceeds the supported depth",
        ));
    }
    Ok(())
}

fn validate_value_at_depth(value: &Value, depth: usize) -> Result<()> {
    check_depth(depth)?;
    match value {
        Value::Object(object) => {
            for (key, value) in object {
                check_key(key)?;
                validate_value_at_depth(value, depth + 1)?;
            }
        }
        Value::Array(values) => {
            for value in values {
                validate_value_at_depth(value, depth + 1)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn validate_raw(raw: &RawValue, depth: usize) -> Result<()> {
    check_depth(depth)?;
    let mut deserializer = serde_json::Deserializer::from_str(raw.get());
    match raw.get().as_bytes().first() {
        Some(b'{') => deserializer.deserialize_map(MemberGuard { depth }),
        Some(b'[') => deserializer.deserialize_seq(MemberGuard { depth }),
        _ => Ok(()),
    }
}

struct MemberGuard {
    depth: usize,
}

impl<'de> Visitor<'de> for MemberGuard {
    type Value = ();

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("JSON without reserved object members")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> std::result::Result<(), A::Error> {
        while let Some(key) = map.next_key::<String>()? {
            check_key(&key).map_err(A::Error::custom)?;
            validate_raw(map.next_value::<&RawValue>()?, self.depth + 1)
                .map_err(A::Error::custom)?;
        }
        Ok(())
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut sequence: A) -> std::result::Result<(), A::Error> {
        while let Some(value) = sequence.next_element::<&RawValue>()? {
            validate_raw(value, self.depth + 1).map_err(A::Error::custom)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reserved_members_are_rejected_before_value_coercion() {
        for text in [
            r#"{"$serde_json::private::Number":"123"}"#,
            r#"{"value":{"$serde_json::private::RawValue":"123"}}"#,
            r#"{"value":[{"\u0024serde_json::private::RawValue":"{\"changed\":true}"}]}"#,
            r#"{"value":[{"\u0024serde_json::private::Number":"123"}]}"#,
            r#"  { "value" : [ { "\u0024serde_json\u003a\u003aprivate::Number" : "123" } ] }  "#,
            r#"{"value":{"other":true,"$serde_json::private::Number":"123"}}"#,
            r#"{"value":{"$serde_json::private::Number":"not a number"},"value":0}"#,
        ] {
            assert!(
                from_str::<Value>(text)
                    .unwrap_err()
                    .to_string()
                    .contains("json.reserved_member")
            );
            assert!(
                from_slice::<Value>(text.as_bytes())
                    .unwrap_err()
                    .to_string()
                    .contains("json.reserved_member")
            );
        }
    }

    #[test]
    fn exact_numbers_and_ordinary_text_survive_checked_parsing() {
        let text = r#"{"large":18446744073709551617,"fraction":0.123456789012345678901,"exponent":1e9999999999999999999999999,"text":"$serde_json::private::Number","quoted":"{\"$serde_json::private::Number\":\"123\"}"}"#;
        let value: Value = from_str(text).unwrap();
        assert_eq!(value["large"].to_string(), "18446744073709551617");
        assert_eq!(value["fraction"].to_string(), "0.123456789012345678901");
        assert_eq!(
            value["exponent"].to_string(),
            "1e+9999999999999999999999999"
        );
        assert_eq!(value["text"], RESERVED_NUMBER_KEY);
        assert_eq!(from_slice::<Value>(text.as_bytes()).unwrap(), value);
        assert!(validate_value(&value).is_ok());
        assert!(
            validate_value(&serde_json::json!({"nested":[{RESERVED_NUMBER_KEY:"123"}]})).is_err()
        );
    }

    #[test]
    fn malformed_or_excessively_nested_json_is_rejected() {
        for text in ["{", "{} trailing", "[1,]", "{\"key\":NaN}"] {
            assert!(from_str::<Value>(text).is_err());
        }
        let text = format!(
            "{}0{}",
            "[".repeat(MAX_DEPTH + 1),
            "]".repeat(MAX_DEPTH + 1)
        );
        assert!(from_str::<Value>(&text).is_err());
    }
}
