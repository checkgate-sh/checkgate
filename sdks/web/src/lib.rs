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

use checkgate_core::evaluator::{Flag, UserContext, evaluate, evaluate_variant};
use checkgate_core::store::FlagStore;
use std::collections::HashMap;
use std::sync::Arc;
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
pub struct CheckgateCoreWasm {
    store: Arc<FlagStore>,
}

impl Default for CheckgateCoreWasm {
    fn default() -> Self {
        Self::new()
    }
}

#[wasm_bindgen]
impl CheckgateCoreWasm {
    #[wasm_bindgen(constructor)]
    pub fn new() -> Self {
        console_error_panic_hook::set_once();
        Self {
            store: Arc::new(FlagStore::new()),
        }
    }

    /// Load a flag from a full JSON string into the browser's Wasm memory.
    /// Accepts all flag fields including flag_type, default_value, disabled_value,
    /// and per-rule variants.
    #[wasm_bindgen]
    pub fn upsert_flag_v2(&self, flag_json: String) {
        if let Ok(flag) = serde_json::from_str::<Flag>(&flag_json) {
            self.store.upsert_flag(flag);
        }
    }

    /// Load a flag directly into the browser's Wasm memory (legacy positional API).
    /// New callers should prefer `upsert_flag_v2`.
    #[wasm_bindgen]
    pub fn upsert_flag(
        &self,
        key: String,
        is_enabled: bool,
        rollout_percentage: Option<u32>,
        description: Option<String>,
        rules_js: JsValue,
    ) {
        let rules: Vec<checkgate_core::evaluator::TargetingRule> =
            serde_wasm_bindgen::from_value(rules_js).unwrap_or_default();
        let flag_json = serde_json::json!({
            "key": key,
            "is_enabled": is_enabled,
            "rollout_percentage": rollout_percentage,
            "description": description,
            "rules": rules,
        });
        if let Ok(flag) = serde_json::from_value::<Flag>(flag_json) {
            self.store.upsert_flag(flag);
        }
    }

    /// Remove a flag from the local cache.
    #[wasm_bindgen]
    pub fn delete_flag(&self, key: String) {
        self.store.delete_flag(&key);
    }

    /// Clear the entire local cache (called on SSE reconnect before re-bootstrap).
    #[wasm_bindgen]
    pub fn clear_store(&self) {
        self.store.clear();
    }

    /// Evaluate a flag and return a boolean.
    #[wasm_bindgen]
    pub fn is_enabled(
        &self,
        flag_key: String,
        user_key: String,
        user_attributes_js: JsValue,
    ) -> bool {
        let flag = match self.store.get_flag(&flag_key) {
            Some(f) => f,
            None => return false,
        };
        let attributes: HashMap<String, String> =
            serde_wasm_bindgen::from_value(user_attributes_js).unwrap_or_default();
        let ctx = UserContext {
            key: user_key,
            attributes,
        };
        evaluate(flag.as_ref(), &ctx, &self.store)
    }

    /// Evaluate a flag and return the resolved value as a JS value.
    /// Returns `null` if the flag is not found.
    #[wasm_bindgen]
    pub fn get_value(
        &self,
        flag_key: String,
        user_key: String,
        user_attributes_js: JsValue,
    ) -> JsValue {
        let flag = match self.store.get_flag(&flag_key) {
            Some(f) => f,
            None => return JsValue::NULL,
        };
        let attributes: HashMap<String, String> =
            serde_wasm_bindgen::from_value(user_attributes_js).unwrap_or_default();
        let ctx = UserContext {
            key: user_key,
            attributes,
        };
        let result = evaluate_variant(flag.as_ref(), &ctx, &self.store);
        serde_wasm_bindgen::to_value(&result.value).unwrap_or(JsValue::NULL)
    }

    /// Evaluate a flag and return the full result `{ enabled, value }` as a JS object.
    /// Returns `null` if the flag is not found.
    #[wasm_bindgen]
    pub fn get_variant(
        &self,
        flag_key: String,
        user_key: String,
        user_attributes_js: JsValue,
    ) -> JsValue {
        let flag = match self.store.get_flag(&flag_key) {
            Some(f) => f,
            None => return JsValue::NULL,
        };
        let attributes: HashMap<String, String> =
            serde_wasm_bindgen::from_value(user_attributes_js).unwrap_or_default();
        let ctx = UserContext {
            key: user_key,
            attributes,
        };
        let result = evaluate_variant(flag.as_ref(), &ctx, &self.store);
        serde_wasm_bindgen::to_value(&result).unwrap_or(JsValue::NULL)
    }
}
