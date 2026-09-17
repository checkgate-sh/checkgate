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

//! React Native JSI — C FFI layer for Checkgate.
//!
//! These `extern "C"` functions are the Rust side of the JSI bridge.
//! The C++ JSI host (`checkgate_jsi.cpp`) calls them synchronously
//! via the generated `checkgate_core.h` header.
//!
//! A global `FlagStore` singleton is used because C FFI functions are
//! stateless — the JS side holds no Rust handles.
//!
//! ## Performance API
//!
//! For hot paths (evaluating multiple flags for the same user), use the context
//! API to parse user attributes **once** and reuse the handle across calls:
//!
//! ```c
//! CheckgateContext* ctx = checkgate_make_context("user-123", "{\"plan\":\"pro\"}");
//! int flag_a = checkgate_is_enabled_ctx("feature-a", ctx);
//! int flag_b = checkgate_is_enabled_ctx("feature-b", ctx);
//! checkgate_free_context(ctx);
//! ```

use checkgate_core::evaluator::{evaluate, evaluate_variant, EvalResult, Flag, UserContext};
use checkgate_core::store::FlagStore;
use std::collections::HashMap;
use std::ffi::{c_char, CStr, CString};
use std::sync::LazyLock;

static STORE: LazyLock<FlagStore> = LazyLock::new(FlagStore::new);

/// Opaque handle holding a pre-parsed user context.
/// Obtain via `checkgate_make_context`, release via `checkgate_free_context`.
pub struct CheckgateContext {
    inner: UserContext,
}

/// Upsert a flag from a full JSON string into the in-memory store.
///
/// Accepts all flag fields including `flag_type`, `default_value`, `disabled_value`,
/// and per-rule `variant` values. Prefer this over `checkgate_upsert_flag` for
/// non-boolean flags.
///
/// # Safety
/// `flag_json` must be a valid, non-dangling, null-terminated C string.
#[no_mangle]
pub unsafe extern "C" fn checkgate_upsert_flag_v2(flag_json: *const c_char) {
    if flag_json.is_null() {
        return;
    }
    // SAFETY: `flag_json` is null-checked directly above, and the caller
    // guarantees (see `# Safety`) that a non-null pointer is a live,
    // NUL-terminated C string. `CStr` borrows it only for this statement.
    let json = unsafe { CStr::from_ptr(flag_json) }.to_string_lossy();
    if let Ok(flag) = serde_json::from_str::<Flag>(&json) {
        STORE.upsert_flag(flag);
    }
}

/// Upsert a flag into the in-memory store (legacy positional API).
///
/// New callers should prefer `checkgate_upsert_flag_v2`.
///
/// # Arguments
/// - `key` — null-terminated flag key
/// - `is_enabled` — global kill-switch
/// - `rollout_percentage` — 0-100, or -1 to mean "no rollout limit" (100%)
/// - `rules_json` — null-terminated JSON array of targeting rules.
///   Pass `"[]"` or NULL when there are no rules.
///
/// # Safety
/// All pointer arguments must be valid, non-dangling, null-terminated C strings.
#[no_mangle]
pub unsafe extern "C" fn checkgate_upsert_flag(
    key: *const c_char,
    is_enabled: bool,
    rollout_percentage: i32,
    rules_json: *const c_char,
) {
    // SAFETY: the caller guarantees (see `# Safety`) that `key` is a live,
    // non-dangling, NUL-terminated C string for the duration of this call.
    let key = unsafe { CStr::from_ptr(key) }
        .to_string_lossy()
        .into_owned();

    let rules: serde_json::Value = if !rules_json.is_null() {
        // SAFETY: this branch runs only when `rules_json` is non-null, and the
        // caller guarantees a non-null pointer is a live, NUL-terminated C string.
        let json = unsafe { CStr::from_ptr(rules_json) }.to_string_lossy();
        serde_json::from_str(&json).unwrap_or(serde_json::Value::Array(vec![]))
    } else {
        serde_json::Value::Array(vec![])
    };

    let rollout = if rollout_percentage < 0 {
        serde_json::Value::Null
    } else {
        serde_json::json!(rollout_percentage.min(100))
    };

    let flag_json = serde_json::json!({
        "key": key,
        "is_enabled": is_enabled,
        "rollout_percentage": rollout,
        "description": null,
        "rules": rules,
    });
    if let Ok(flag) = serde_json::from_value::<Flag>(flag_json) {
        STORE.upsert_flag(flag);
    }
}

/// Remove a flag from the in-memory store.
///
/// # Safety
/// `key` must be a valid, non-dangling, null-terminated C string.
#[no_mangle]
pub unsafe extern "C" fn checkgate_delete_flag(key: *const c_char) {
    // SAFETY: the caller guarantees (see `# Safety`) that `key` is a live,
    // non-dangling, NUL-terminated C string for the duration of this call.
    let key = unsafe { CStr::from_ptr(key) }.to_string_lossy();
    STORE.delete_flag(&key);
}

/// Clear all flags from the in-memory store (called on SSE reconnect).
#[no_mangle]
pub extern "C" fn checkgate_clear_store() {
    STORE.clear();
}

// ---------------------------------------------------------------------------
// Context-based API — parse attributes once, evaluate many flags cheaply
// ---------------------------------------------------------------------------

/// Parse a user key and attributes JSON into an opaque context handle.
///
/// Returns a heap-allocated pointer the caller owns.
/// Must be released with `checkgate_free_context`.
///
/// # Safety
/// All pointer arguments must be valid, non-dangling, null-terminated C strings.
#[no_mangle]
pub unsafe extern "C" fn checkgate_make_context(
    user_key: *const c_char,
    attributes_json: *const c_char,
) -> *mut CheckgateContext {
    // SAFETY: the caller guarantees (see `# Safety`) that `user_key` is a live,
    // non-dangling, NUL-terminated C string for the duration of this call.
    let user_key = unsafe { CStr::from_ptr(user_key) }
        .to_string_lossy()
        .into_owned();

    let attributes: HashMap<String, String> = if !attributes_json.is_null() {
        // SAFETY: this branch runs only when `attributes_json` is non-null, and the
        // caller guarantees a non-null pointer is a live, NUL-terminated C string.
        let json = unsafe { CStr::from_ptr(attributes_json) }.to_string_lossy();
        serde_json::from_str(&json).unwrap_or_default()
    } else {
        HashMap::new()
    };

    Box::into_raw(Box::new(CheckgateContext {
        inner: UserContext {
            key: user_key,
            attributes,
        },
    }))
}

/// Evaluate a flag using a pre-parsed context handle. Returns `1` if enabled, `0` otherwise.
///
/// # Safety
/// - `flag_key` must be a valid, non-dangling, null-terminated C string.
/// - `ctx`, if non-null, must be a pointer from `checkgate_make_context` that has
///   not been freed. Passing NULL is safe and evaluates to `0` (fail closed).
#[no_mangle]
pub unsafe extern "C" fn checkgate_is_enabled_ctx(
    flag_key: *const c_char,
    ctx: *const CheckgateContext,
) -> i32 {
    // SAFETY: the caller guarantees (see `# Safety`) that `flag_key` is a live,
    // non-dangling, NUL-terminated C string for the duration of this call.
    let flag_key = unsafe { CStr::from_ptr(flag_key) }.to_string_lossy();
    let flag = match STORE.get_flag(&flag_key) {
        Some(f) => f,
        None => return 0,
    };
    if ctx.is_null() {
        return 0;
    }
    // SAFETY: `ctx` is null-checked directly above, and the caller guarantees
    // (see `# Safety`) that a non-null `ctx` came from `checkgate_make_context`
    // without an intervening `checkgate_free_context` — so it points to a live,
    // aligned, initialised `CheckgateContext`. The borrow ends with this call.
    let ctx = unsafe { &*ctx };
    if evaluate(flag.as_ref(), &ctx.inner, &STORE) {
        1
    } else {
        0
    }
}

/// Release a context handle. Passing NULL is safe and is a no-op.
///
/// # Safety
/// `ctx` must be a pointer from `checkgate_make_context` that has not already been freed.
#[no_mangle]
pub unsafe extern "C" fn checkgate_free_context(ctx: *mut CheckgateContext) {
    if !ctx.is_null() {
        // SAFETY: `ctx` is null-checked directly above. A non-null `ctx` originates
        // from `Box::into_raw` in `checkgate_make_context`, so rebuilding the `Box`
        // with the same layout is the matching deallocation. The caller's `# Safety`
        // contract forbids passing it twice.
        drop(unsafe { Box::from_raw(ctx) });
    }
}

// ---------------------------------------------------------------------------
// Variant API — returns value alongside enabled/disabled result
// ---------------------------------------------------------------------------

fn alloc_cstring(s: String) -> *mut c_char {
    // `s` only fails to convert if it contains an interior NUL. The `c"null"`
    // fallback is a literal, so this path allocates without any chance of panic.
    CString::new(s)
        .unwrap_or_else(|_| c"null".to_owned())
        .into_raw()
}

fn eval_result_to_json(result: EvalResult) -> String {
    serde_json::to_string(&result).unwrap_or_else(|_| "null".to_string())
}

fn value_to_json(result: EvalResult) -> String {
    serde_json::to_string(&result.value).unwrap_or_else(|_| "null".to_string())
}

/// Evaluate a flag and return a heap-allocated JSON string `{"enabled":bool,"value":...}`.
/// Returns `"null"` if the flag is not found.
///
/// The caller must free the returned pointer with `checkgate_free_string`.
///
/// # Safety
/// All pointer arguments must be valid, non-dangling, null-terminated C strings.
#[no_mangle]
pub unsafe extern "C" fn checkgate_get_variant(
    flag_key: *const c_char,
    user_key: *const c_char,
    attributes_json: *const c_char,
) -> *mut c_char {
    // SAFETY: the caller guarantees (see `# Safety`) that `flag_key` is a live,
    // non-dangling, NUL-terminated C string for the duration of this call.
    let flag_key = unsafe { CStr::from_ptr(flag_key) }.to_string_lossy();
    let flag = match STORE.get_flag(&flag_key) {
        Some(f) => f,
        None => return alloc_cstring("null".to_string()),
    };
    // SAFETY: the caller guarantees (see `# Safety`) that `user_key` is a live,
    // non-dangling, NUL-terminated C string for the duration of this call.
    let user_key = unsafe { CStr::from_ptr(user_key) }
        .to_string_lossy()
        .into_owned();
    let attributes: HashMap<String, String> = if !attributes_json.is_null() {
        // SAFETY: this branch runs only when `attributes_json` is non-null, and the
        // caller guarantees a non-null pointer is a live, NUL-terminated C string.
        let json = unsafe { CStr::from_ptr(attributes_json) }.to_string_lossy();
        serde_json::from_str(&json).unwrap_or_default()
    } else {
        HashMap::new()
    };
    let ctx = UserContext {
        key: user_key,
        attributes,
    };
    alloc_cstring(eval_result_to_json(evaluate_variant(
        flag.as_ref(),
        &ctx,
        &STORE,
    )))
}

/// Evaluate a flag and return a heap-allocated JSON string of just the variant value.
/// Returns `"null"` if the flag is not found.
///
/// The caller must free the returned pointer with `checkgate_free_string`.
///
/// # Safety
/// All pointer arguments must be valid, non-dangling, null-terminated C strings.
#[no_mangle]
pub unsafe extern "C" fn checkgate_get_value(
    flag_key: *const c_char,
    user_key: *const c_char,
    attributes_json: *const c_char,
) -> *mut c_char {
    // SAFETY: the caller guarantees (see `# Safety`) that `flag_key` is a live,
    // non-dangling, NUL-terminated C string for the duration of this call.
    let flag_key = unsafe { CStr::from_ptr(flag_key) }.to_string_lossy();
    let flag = match STORE.get_flag(&flag_key) {
        Some(f) => f,
        None => return alloc_cstring("null".to_string()),
    };
    // SAFETY: the caller guarantees (see `# Safety`) that `user_key` is a live,
    // non-dangling, NUL-terminated C string for the duration of this call.
    let user_key = unsafe { CStr::from_ptr(user_key) }
        .to_string_lossy()
        .into_owned();
    let attributes: HashMap<String, String> = if !attributes_json.is_null() {
        // SAFETY: this branch runs only when `attributes_json` is non-null, and the
        // caller guarantees a non-null pointer is a live, NUL-terminated C string.
        let json = unsafe { CStr::from_ptr(attributes_json) }.to_string_lossy();
        serde_json::from_str(&json).unwrap_or_default()
    } else {
        HashMap::new()
    };
    let ctx = UserContext {
        key: user_key,
        attributes,
    };
    alloc_cstring(value_to_json(evaluate_variant(flag.as_ref(), &ctx, &STORE)))
}

/// Free a string returned by `checkgate_get_variant` or `checkgate_get_value`.
/// Passing NULL is safe and is a no-op.
///
/// # Safety
/// `s` must be a pointer from `checkgate_get_variant` or `checkgate_get_value`
/// that has not already been freed.
#[no_mangle]
pub unsafe extern "C" fn checkgate_free_string(s: *mut c_char) {
    if !s.is_null() {
        // SAFETY: `s` is null-checked directly above. A non-null `s` originates from
        // `CString::into_raw` in `alloc_cstring`, so rebuilding the `CString` is the
        // matching deallocation. The caller's `# Safety` contract forbids a double free.
        drop(unsafe { CString::from_raw(s) });
    }
}

// ---------------------------------------------------------------------------
// Legacy single-call API (parses attributes JSON on every invocation)
// ---------------------------------------------------------------------------

/// Evaluate a flag for a given user. Parses `attributes_json` on every call.
///
/// Prefer `checkgate_make_context` + `checkgate_is_enabled_ctx` when evaluating
/// multiple flags for the same user.
///
/// # Returns `1` if enabled, `0` otherwise.
///
/// # Safety
/// All pointer arguments must be valid, non-dangling, null-terminated C strings.
#[no_mangle]
pub unsafe extern "C" fn checkgate_is_enabled(
    flag_key: *const c_char,
    user_key: *const c_char,
    attributes_json: *const c_char,
) -> i32 {
    // SAFETY: the caller guarantees (see `# Safety`) that `flag_key` is a live,
    // non-dangling, NUL-terminated C string for the duration of this call.
    let flag_key = unsafe { CStr::from_ptr(flag_key) }.to_string_lossy();
    // SAFETY: the caller guarantees (see `# Safety`) that `user_key` is a live,
    // non-dangling, NUL-terminated C string for the duration of this call.
    let user_key = unsafe { CStr::from_ptr(user_key) }
        .to_string_lossy()
        .into_owned();

    let flag = match STORE.get_flag(&flag_key) {
        Some(f) => f,
        None => return 0,
    };

    let attributes: HashMap<String, String> = if !attributes_json.is_null() {
        // SAFETY: this branch runs only when `attributes_json` is non-null, and the
        // caller guarantees a non-null pointer is a live, NUL-terminated C string.
        let json = unsafe { CStr::from_ptr(attributes_json) }.to_string_lossy();
        serde_json::from_str(&json).unwrap_or_default()
    } else {
        HashMap::new()
    };

    let ctx = UserContext {
        key: user_key,
        attributes,
    };
    if evaluate(flag.as_ref(), &ctx, &STORE) {
        1
    } else {
        0
    }
}

// ---------------------------------------------------------------------------
// Tests
//
// These drive the FFI surface through real raw pointers rather than calling the
// safe Rust underneath, which is the point: it gives `cargo +nightly miri test`
// something to check. Miri validates the `Box::into_raw`/`from_raw` and
// `CString::into_raw`/`from_raw` round trips, pointer provenance, and that no
// allocation is leaked or freed twice.
//
// `STORE` is a process-wide singleton and tests run in parallel, so every test
// uses flag keys unique to itself instead of calling `checkgate_clear_store`.
// ---------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;

    fn cs(s: &str) -> CString {
        CString::new(s).expect("test literal has no interior NUL")
    }

    /// Upsert a simple always-on boolean flag under `key`.
    fn upsert_on(key: &str) {
        let json = cs(&format!(
            r#"{{"key":"{key}","is_enabled":true,"rollout_percentage":100,"rules":[]}}"#
        ));
        // SAFETY: `json` is a live `CString` owned by this frame for the whole call.
        unsafe { checkgate_upsert_flag_v2(json.as_ptr()) };
    }

    #[test]
    fn upsert_then_evaluate_without_a_context() {
        let key = "rn-eval-no-ctx";
        upsert_on(key);
        let (k, u) = (cs(key), cs("user-1"));
        // SAFETY: both pointers come from live `CString`s; attributes is NULL,
        // which the function documents as "no attributes".
        let got = unsafe { checkgate_is_enabled(k.as_ptr(), u.as_ptr(), std::ptr::null()) };
        assert_eq!(got, 1);
    }

    #[test]
    fn context_handle_round_trips_through_raw_pointers() {
        let key = "rn-eval-ctx";
        upsert_on(key);
        let (u, attrs) = (cs("user-2"), cs(r#"{"plan":"pro"}"#));
        // SAFETY: both pointers are live for the call; the returned handle is
        // owned by this frame and released by `checkgate_free_context` below.
        let ctx = unsafe { checkgate_make_context(u.as_ptr(), attrs.as_ptr()) };
        assert!(!ctx.is_null());

        let k = cs(key);
        // SAFETY: `k` is live and `ctx` is the unfreed handle made just above.
        let got = unsafe { checkgate_is_enabled_ctx(k.as_ptr(), ctx) };
        assert_eq!(got, 1);

        // SAFETY: `ctx` came from `checkgate_make_context` and is freed exactly once.
        unsafe { checkgate_free_context(ctx) };
    }

    #[test]
    fn null_context_fails_closed_instead_of_dereferencing() {
        let k = cs("rn-null-ctx");
        // SAFETY: `k` is live; passing a NULL `ctx` is explicitly supported.
        let got = unsafe { checkgate_is_enabled_ctx(k.as_ptr(), std::ptr::null()) };
        assert_eq!(got, 0, "a NULL context must fail closed, not segfault");
    }

    #[test]
    fn returned_strings_round_trip_and_are_freed() {
        let key = "rn-variant";
        upsert_on(key);
        let (k, u) = (cs(key), cs("user-3"));
        // SAFETY: both pointers are live; the result is an owned C string that
        // this test hands back to `checkgate_free_string`.
        let out = unsafe { checkgate_get_variant(k.as_ptr(), u.as_ptr(), std::ptr::null()) };
        assert!(!out.is_null());

        // SAFETY: `out` is the live, NUL-terminated string just returned.
        let text = unsafe { CStr::from_ptr(out) }
            .to_string_lossy()
            .into_owned();
        assert!(text.contains("enabled"), "unexpected payload: {text}");

        // SAFETY: `out` came from `alloc_cstring` and is freed exactly once.
        unsafe { checkgate_free_string(out) };
    }

    #[test]
    fn free_string_and_free_context_tolerate_null() {
        // SAFETY: both functions document NULL as a supported no-op.
        unsafe {
            checkgate_free_string(std::ptr::null_mut());
            checkgate_free_context(std::ptr::null_mut());
        }
    }

    #[test]
    fn delete_removes_the_flag_and_evaluation_fails_closed() {
        let key = "rn-delete";
        upsert_on(key);
        let (k, u) = (cs(key), cs("user-4"));
        // SAFETY: `k` is live for the duration of the call.
        unsafe { checkgate_delete_flag(k.as_ptr()) };
        // SAFETY: both pointers are live; the flag is now absent.
        let got = unsafe { checkgate_is_enabled(k.as_ptr(), u.as_ptr(), std::ptr::null()) };
        assert_eq!(got, 0);
    }
}
