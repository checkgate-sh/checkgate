// `deny` rather than `forbid`: the #[napi] / #[wasm_bindgen] proc macros expand
// to unsafe glue carrying their own `allow(unsafe_code)`, and `forbid` cannot be
// overridden even by generated code. `deny` still rejects hand-written unsafe.
#![deny(unsafe_code)]
#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing,
        clippy::arithmetic_side_effects
    )
)]
#![deny(clippy::all)]

#[macro_use]
extern crate napi_derive;

use checkgate_core::evaluator::{Flag, UserContext, evaluate, evaluate_variant};
use checkgate_core::store::FlagStore;
use napi::bindgen_prelude::{JsObjectValue, Object};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// Builds the evaluation context, converting only the attributes the flag can
/// actually read: those named by its own rules and, transitively, by the rules
/// of its prerequisites (each prerequisite flag visited once).
///
/// Converting the caller's whole attribute object into a `HashMap` cost about
/// 1.3 µs per attribute on every call — most of a ~7 µs `isEnabled` with five
/// attributes, spent before the flag was even looked up, and paid in full by
/// flags that have no rules at all. Reading only referenced names gives the
/// evaluator exactly the entries it would have looked up in the full map, so
/// every result is unchanged.
///
/// Semantics kept from the full conversion: an attribute whose value is
/// `undefined` is absent, and a referenced attribute that is not a string
/// throws the same conversion error as before. Differences: attributes no rule references are no
/// longer validated (a stray number elsewhere in the object no longer throws),
/// only own properties are read, and `null`/`undefined` in place of the object
/// means "no attributes".
fn user_context(
    flag: &Flag,
    store: &FlagStore,
    user_key: String,
    user_attributes: Option<Object<'_>>,
) -> napi::Result<UserContext> {
    let mut attributes = HashMap::new();
    if let Some(object) = user_attributes {
        let mut read = |name: &str| -> napi::Result<()> {
            if attributes.contains_key(name) || !object.has_own_property(name)? {
                return Ok(());
            }
            if let Some(value) = object.get::<String>(name)? {
                attributes.insert(name.to_owned(), value);
            }
            Ok(())
        };
        for rule in &flag.rules {
            read(&rule.attribute)?;
        }
        if !flag.prerequisites.is_empty() {
            let mut seen = HashSet::from([flag.key.clone()]);
            let mut pending: Vec<Arc<Flag>> = Vec::new();
            let mut enqueue = |from: &Flag, pending: &mut Vec<Arc<Flag>>| {
                for prerequisite in &from.prerequisites {
                    if seen.insert(prerequisite.flag_key.clone())
                        && let Some(found) = store.get_flag(&prerequisite.flag_key)
                    {
                        pending.push(found);
                    }
                }
            };
            enqueue(flag, &mut pending);
            while let Some(prerequisite) = pending.pop() {
                for rule in &prerequisite.rules {
                    read(&rule.attribute)?;
                }
                enqueue(&prerequisite, &mut pending);
            }
        }
    }
    Ok(UserContext {
        key: user_key,
        attributes,
    })
}

#[napi]
pub struct CheckgateCore {
    store: Arc<FlagStore>,
}

impl Default for CheckgateCore {
    fn default() -> Self {
        Self::new()
    }
}

#[napi]
impl CheckgateCore {
    #[napi(constructor)]
    pub fn new() -> Self {
        Self {
            store: Arc::new(FlagStore::new()),
        }
    }

    /// Load a flag from a full JSON string into the in-memory cache.
    /// Accepts all flag fields including flag_type, default_value, disabled_value,
    /// and per-rule variants.
    #[napi]
    pub fn upsert_flag_v2(&self, flag_json: String) {
        if let Ok(flag) = serde_json::from_str::<Flag>(&flag_json) {
            self.store.upsert_flag(flag);
        }
    }

    /// Load a flag directly into the in-memory cache (legacy positional API).
    /// New callers should prefer `upsert_flag_v2`.
    #[napi]
    pub fn upsert_flag(
        &self,
        key: String,
        is_enabled: bool,
        rollout_percentage: Option<u32>,
        description: Option<String>,
        rules_json: Option<String>,
    ) {
        let flag_json = serde_json::json!({
            "key": key,
            "is_enabled": is_enabled,
            "rollout_percentage": rollout_percentage,
            "description": description,
            "rules": rules_json
                .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
                .unwrap_or(serde_json::Value::Array(vec![])),
        });
        if let Ok(flag) = serde_json::from_value::<Flag>(flag_json) {
            self.store.upsert_flag(flag);
        }
    }

    /// Remove a flag from the local cache.
    #[napi]
    pub fn delete_flag(&self, key: String) {
        self.store.delete_flag(&key);
    }

    /// Clear the entire local cache (called on SSE reconnect before re-bootstrap).
    #[napi]
    pub fn clear_store(&self) {
        self.store.clear();
    }

    /// Evaluate a flag and return a boolean.
    #[napi]
    pub fn is_enabled(
        &self,
        flag_key: String,
        user_key: String,
        user_attributes: Option<Object<'_>>,
    ) -> napi::Result<bool> {
        let Some(flag) = self.store.get_flag(&flag_key) else {
            return Ok(false);
        };
        let ctx = user_context(&flag, &self.store, user_key, user_attributes)?;
        Ok(evaluate(flag.as_ref(), &ctx, &self.store))
    }

    /// Evaluate a flag and return the resolved variant value as a JSON string.
    /// Returns `null` (as JSON string "null") if the flag is not found.
    #[napi]
    pub fn get_value(
        &self,
        flag_key: String,
        user_key: String,
        user_attributes: Option<Object<'_>>,
    ) -> napi::Result<String> {
        let Some(flag) = self.store.get_flag(&flag_key) else {
            return Ok("null".to_string());
        };
        let ctx = user_context(&flag, &self.store, user_key, user_attributes)?;
        let result = evaluate_variant(flag.as_ref(), &ctx, &self.store);
        Ok(serde_json::to_string(&result.value).unwrap_or_else(|_| "null".to_string()))
    }

    /// Evaluate a flag and return the full result `{ enabled, value }` as a JSON string.
    /// Returns `null` (as JSON string "null") if the flag is not found.
    #[napi]
    pub fn get_variant(
        &self,
        flag_key: String,
        user_key: String,
        user_attributes: Option<Object<'_>>,
    ) -> napi::Result<String> {
        let Some(flag) = self.store.get_flag(&flag_key) else {
            return Ok("null".to_string());
        };
        let ctx = user_context(&flag, &self.store, user_key, user_attributes)?;
        let result = evaluate_variant(flag.as_ref(), &ctx, &self.store);
        Ok(serde_json::to_string(&result).unwrap_or_else(|_| "null".to_string()))
    }
}
