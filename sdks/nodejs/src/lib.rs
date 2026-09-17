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
use std::collections::HashMap;
use std::sync::Arc;

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
        user_attributes: HashMap<String, String>,
    ) -> bool {
        let flag = match self.store.get_flag(&flag_key) {
            Some(f) => f,
            None => return false,
        };
        let ctx = UserContext {
            key: user_key,
            attributes: user_attributes,
        };
        evaluate(flag.as_ref(), &ctx, &self.store)
    }

    /// Evaluate a flag and return the resolved variant value as a JSON string.
    /// Returns `null` (as JSON string "null") if the flag is not found.
    #[napi]
    pub fn get_value(
        &self,
        flag_key: String,
        user_key: String,
        user_attributes: HashMap<String, String>,
    ) -> String {
        let flag = match self.store.get_flag(&flag_key) {
            Some(f) => f,
            None => return "null".to_string(),
        };
        let ctx = UserContext {
            key: user_key,
            attributes: user_attributes,
        };
        let result = evaluate_variant(flag.as_ref(), &ctx, &self.store);
        serde_json::to_string(&result.value).unwrap_or_else(|_| "null".to_string())
    }

    /// Evaluate a flag and return the full result `{ enabled, value }` as a JSON string.
    /// Returns `null` (as JSON string "null") if the flag is not found.
    #[napi]
    pub fn get_variant(
        &self,
        flag_key: String,
        user_key: String,
        user_attributes: HashMap<String, String>,
    ) -> String {
        let flag = match self.store.get_flag(&flag_key) {
            Some(f) => f,
            None => return "null".to_string(),
        };
        let ctx = UserContext {
            key: user_key,
            attributes: user_attributes,
        };
        let result = evaluate_variant(flag.as_ref(), &ctx, &self.store);
        serde_json::to_string(&result).unwrap_or_else(|_| "null".to_string())
    }
}
