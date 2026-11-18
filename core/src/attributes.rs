//! Decoding user attributes handed over by a host language.
//!
//! Attributes are string-valued by contract, but bindings receive them from
//! dynamically typed callers (`{ plan: "pro", age: 30 }` from JavaScript,
//! `Map<String, dynamic>` from Dart). Decoding straight into
//! `HashMap<String, String>` fails on the first non-string value, and the
//! bindings used to answer that failure with an *empty* map — every attribute
//! dropped. Because a missing attribute satisfies `not_equals`, that turned a
//! user who should be excluded (`plan not_equals free` with `plan: "free"`)
//! into one who is targeted.
//!
//! [`string_attributes`] keeps every string value and skips only the values
//! that are not strings, so one bad attribute no longer erases the others.

use serde::de::{Deserializer, IgnoredAny};
use serde::Deserialize;
use std::collections::HashMap;

/// A value that is kept only if it is a string.
#[derive(Deserialize)]
#[serde(untagged)]
enum MaybeString {
    Str(String),
    Other(IgnoredAny),
}

/// Deserializes an object of attributes, keeping string values and skipping
/// any other value (numbers, booleans, null, nested objects). Fails only when
/// the input is not an object at all.
pub fn string_attributes<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<HashMap<String, String>, D::Error> {
    let raw = HashMap::<String, MaybeString>::deserialize(deserializer)?;
    Ok(raw
        .into_iter()
        .filter_map(|(key, value)| match value {
            MaybeString::Str(s) => Some((key, s)),
            MaybeString::Other(_) => None,
        })
        .collect())
}

/// [`string_attributes`] over a JSON document. Input that is not a JSON object
/// yields no attributes, as before.
pub fn attributes_from_json(json: &str) -> HashMap<String, String> {
    string_attributes(&mut serde_json::Deserializer::from_str(json)).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_strings_and_skips_only_the_invalid_values() {
        let attrs =
            attributes_from_json(r#"{"plan":"free","age":30,"beta":true,"x":null,"o":{"a":1}}"#);
        assert_eq!(attrs.len(), 1);
        assert_eq!(attrs.get("plan").map(String::as_str), Some("free"));
    }

    #[test]
    fn the_old_strict_decoding_lost_every_attribute() {
        // What the bindings did before: one number empties the whole map.
        let strict: HashMap<String, String> =
            serde_json::from_str(r#"{"plan":"free","age":30}"#).unwrap_or_default();
        assert!(strict.is_empty());
        assert_eq!(attributes_from_json(r#"{"plan":"free","age":30}"#).len(), 1);
    }

    #[test]
    fn non_objects_yield_no_attributes() {
        for input in ["", "null", "[]", "\"plan\"", "42", "{not json"] {
            assert!(attributes_from_json(input).is_empty(), "{input:?}");
        }
    }

    #[test]
    fn excluded_user_stays_excluded_when_another_attribute_is_invalid() {
        use crate::evaluator::{evaluate, Flag, FlagType, Operator, TargetingRule, UserContext};
        use crate::store::FlagStore;
        let flag = Flag {
            key: "paid-only".into(),
            is_enabled: true,
            rollout_percentage: Some(0),
            description: None,
            rules: vec![TargetingRule {
                attribute: "plan".into(),
                operator: Operator::NotEquals,
                values: vec!["free".into()],
                segment_key: None,
                variant: None,
            }],
            flag_type: FlagType::Boolean,
            default_value: None,
            disabled_value: None,
            variants: vec![],
            prerequisites: vec![],
        };
        let ctx = UserContext {
            key: "u".into(),
            attributes: attributes_from_json(r#"{"plan":"free","age":30}"#),
        };
        assert!(!evaluate(&flag, &ctx, &FlagStore::new()));
    }
}
