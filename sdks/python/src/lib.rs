//! `checkgate._core`: the Checkgate evaluation engine for CPython.
//!
//! Evaluation only. Syncing flags from the server (SSE, polling, revisions,
//! persistence) is pure Python in `checkgate._client`; this module holds the
//! flag set and evaluates it with `checkgate-core`, the same code every other
//! SDK and the server run.
//!
//! # Concurrency
//!
//! One `Engine` is shared by every Python thread of a process. Evaluations
//! read the current flag set through an `Arc` taken under a read lock held
//! only for that clone, then evaluate without the GIL. Writers build the next
//! flag set beside the live one and swap it in whole (copy-on-write, see
//! [`FlagStore::independent_copy`]), so an evaluation sees the set before a
//! change or the set after it, never a mix of the two: a flag and its
//! prerequisites always come from one version. Writers are serialized by
//! `writer`; lock order is `writer` then `current`, and no Python code runs
//! while either is held.

// `deny` rather than `forbid`: the #[pyclass] / #[pymethods] / #[pymodule]
// proc macros expand to unsafe glue carrying their own `allow(unsafe_code)`,
// and `forbid` cannot be overridden even by generated code. `deny` still
// rejects hand-written unsafe.
#![deny(unsafe_code)]
#![deny(clippy::all)]

use checkgate_core::evaluator::{Flag, UserContext, evaluate, evaluate_variant, explain};
use checkgate_core::store::FlagStore;
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList, PyString};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, PoisonError, RwLock};

/// Builds the evaluation context, reading only the attributes the flag can
/// actually use: those named by its own rules and, transitively, by the rules
/// of its prerequisites (each prerequisite flag visited once).
///
/// Values are kept when they are strings (`str` or a subclass) and skipped
/// otherwise, exactly as `checkgate_core::attributes::string_attributes`
/// decodes attributes for the web, React Native and Flutter SDKs: one
/// non-string value never erases the others. (The Node SDK throws instead.)
/// Reading only referenced names gives the evaluator exactly the entries it
/// would have looked up in a full conversion, so results are unchanged.
fn user_context(
    flag: &Flag,
    store: &FlagStore,
    user_key: &str,
    attributes: Option<&Bound<'_, PyDict>>,
) -> PyResult<UserContext> {
    let mut read = HashMap::new();
    if let Some(dict) = attributes.filter(|d| !d.is_empty()) {
        let mut take = |name: &str| -> PyResult<()> {
            if read.contains_key(name) {
                return Ok(());
            }
            if let Some(value) = dict.get_item(name)?
                && let Ok(text) = value.cast::<PyString>()
            {
                read.insert(name.to_owned(), text.to_str()?.to_owned());
            }
            Ok(())
        };
        for rule in &flag.rules {
            take(&rule.attribute)?;
        }
        if !flag.prerequisites.is_empty() {
            let mut seen = HashSet::from([flag.key.clone()]);
            let mut pending: Vec<Arc<Flag>> = Vec::new();
            let enqueue =
                |from: &Flag, seen: &mut HashSet<String>, pending: &mut Vec<Arc<Flag>>| {
                    for prerequisite in &from.prerequisites {
                        if seen.insert(prerequisite.flag_key.clone())
                            && let Some(found) = store.get_flag(&prerequisite.flag_key)
                        {
                            pending.push(found);
                        }
                    }
                };
            enqueue(flag, &mut seen, &mut pending);
            while let Some(prerequisite) = pending.pop() {
                for rule in &prerequisite.rules {
                    take(&rule.attribute)?;
                }
                enqueue(&prerequisite, &mut seen, &mut pending);
            }
        }
    }
    Ok(UserContext {
        key: user_key.to_owned(),
        attributes: read,
    })
}

/// A JSON value as the equivalent Python object (`dict`, `list`, `str`,
/// `int`, `float`, `bool`, `None`). Depth is bounded by serde_json's parse
/// recursion limit (128): every value here came from a parsed flag.
fn json_to_py<'py>(py: Python<'py>, value: &serde_json::Value) -> PyResult<Bound<'py, PyAny>> {
    use serde_json::Value;
    Ok(match value {
        Value::Null => py.None().into_bound(py),
        Value::Bool(b) => b.into_pyobject(py)?.to_owned().into_any(),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                i.into_pyobject(py)?.into_any()
            } else if let Some(u) = n.as_u64() {
                u.into_pyobject(py)?.into_any()
            } else {
                n.as_f64().unwrap_or(f64::NAN).into_pyobject(py)?.into_any()
            }
        }
        Value::String(s) => PyString::new(py, s).into_any(),
        Value::Array(items) => {
            let list = PyList::empty(py);
            for item in items {
                list.append(json_to_py(py, item)?)?;
            }
            list.into_any()
        }
        Value::Object(map) => {
            let dict = PyDict::new(py);
            for (k, v) in map {
                dict.set_item(k, json_to_py(py, v)?)?;
            }
            dict.into_any()
        }
    })
}

fn to_py<'py, T: serde::Serialize>(py: Python<'py>, value: &T) -> PyResult<Bound<'py, PyAny>> {
    let json = serde_json::to_value(value)
        .map_err(|e| PyValueError::new_err(format!("cannot represent the result: {e}")))?;
    json_to_py(py, &json)
}

/// Parses one flag as served by the Checkgate server. The error names what
/// was wrong (serde's message), never echoes the document.
fn parse_flag(json: &str) -> Result<Flag, String> {
    serde_json::from_str::<Flag>(json).map_err(|e| e.to_string())
}

/// The Checkgate evaluation engine: a flag set and the evaluator.
///
/// Thread-safe: share one instance between threads. Evaluations release the
/// GIL while they run and never see a partially applied change.
#[pyclass(frozen, module = "checkgate._core")]
struct Engine {
    /// The live flag set. Replaced whole on every change, never mutated
    /// while it is live.
    current: RwLock<Arc<FlagStore>>,
    /// Serializes writers (copy, change, swap).
    writer: Mutex<()>,
}

impl Engine {
    /// The live flag set. A poisoned lock still holds a valid `Arc` (the
    /// only write is one pointer assignment), so poisoning is recovered.
    fn snapshot(&self) -> Arc<FlagStore> {
        Arc::clone(&self.current.read().unwrap_or_else(PoisonError::into_inner))
    }

    /// Applies `change` to a copy of the live set and swaps the copy in.
    fn commit<T>(&self, change: impl FnOnce(&FlagStore) -> T) -> T {
        let _writer = self.writer.lock().unwrap_or_else(PoisonError::into_inner);
        let next = self.snapshot().independent_copy();
        let out = change(&next);
        *self.current.write().unwrap_or_else(PoisonError::into_inner) = Arc::new(next);
        out
    }
}

#[pymethods]
impl Engine {
    #[new]
    fn new() -> Self {
        Self {
            current: RwLock::new(Arc::new(FlagStore::new())),
            writer: Mutex::new(()),
        }
    }

    /// Loads one flag (JSON as served by the server). Returns False, and
    /// changes nothing, if it is not a valid flag.
    fn upsert_flag_json(&self, py: Python<'_>, flag_json: &str) -> bool {
        py.detach(|| match parse_flag(flag_json) {
            Ok(flag) => {
                self.commit(|store| store.upsert_flag(flag));
                true
            }
            Err(_) => false,
        })
    }

    /// Raises ValueError (with the parse error) if `flag_json` is not a
    /// valid flag; returns its key otherwise. Loads nothing.
    fn validate_flag_json(&self, py: Python<'_>, flag_json: &str) -> PyResult<String> {
        py.detach(|| parse_flag(flag_json).map(|f| f.key))
            .map_err(PyValueError::new_err)
    }

    /// Removes a flag. Returns whether it was held.
    fn delete_flag(&self, py: Python<'_>, key: &str) -> bool {
        py.detach(|| {
            self.commit(|store| {
                let held = store.get_flag(key).is_some();
                store.delete_flag(key);
                held
            })
        })
    }

    /// Removes every flag.
    fn clear(&self, py: Python<'_>) {
        py.detach(|| {
            let _writer = self.writer.lock().unwrap_or_else(PoisonError::into_inner);
            *self.current.write().unwrap_or_else(PoisonError::into_inner) =
                Arc::new(FlagStore::new());
        });
    }

    /// Applies a batch atomically: removes `deletes`, then loads every
    /// `(key, flag_json)` of `upserts`. Concurrent evaluations see the set
    /// before the batch or after it, never part of it.
    ///
    /// An upsert that is not a valid flag, or whose JSON names a different
    /// key, is not loaded and its key is removed (fail closed: the server
    /// replaced that flag, so the old version must not keep serving). Returns
    /// the keys rejected that way.
    fn apply(
        &self,
        py: Python<'_>,
        upserts: Vec<(String, String)>,
        deletes: Vec<String>,
    ) -> Vec<String> {
        py.detach(|| {
            let parsed: Vec<(String, Result<Flag, String>)> = upserts
                .into_iter()
                .map(|(key, json)| {
                    let flag = parse_flag(&json).and_then(|f| {
                        if f.key == key {
                            Ok(f)
                        } else {
                            Err("key mismatch".to_owned())
                        }
                    });
                    (key, flag)
                })
                .collect();
            self.commit(|store| {
                for key in &deletes {
                    store.delete_flag(key);
                }
                let mut rejected = Vec::new();
                for (key, flag) in parsed {
                    match flag {
                        Ok(flag) => store.upsert_flag(flag),
                        Err(_) => {
                            store.delete_flag(&key);
                            rejected.push(key);
                        }
                    }
                }
                rejected
            })
        })
    }

    /// Whether the flag is on for this user. False if the flag is unknown.
    #[pyo3(signature = (flag_key, user_key, attributes=None))]
    fn is_enabled(
        &self,
        py: Python<'_>,
        flag_key: &str,
        user_key: &str,
        attributes: Option<&Bound<'_, PyDict>>,
    ) -> PyResult<bool> {
        let store = self.snapshot();
        let Some(flag) = store.get_flag(flag_key) else {
            return Ok(false);
        };
        let ctx = user_context(&flag, &store, user_key, attributes)?;
        Ok(py.detach(|| evaluate(&flag, &ctx, &store)))
    }

    /// `(enabled, value)` for this user, or None if the flag is unknown.
    #[pyo3(signature = (flag_key, user_key, attributes=None))]
    fn get_variant<'py>(
        &self,
        py: Python<'py>,
        flag_key: &str,
        user_key: &str,
        attributes: Option<&Bound<'py, PyDict>>,
    ) -> PyResult<Option<(bool, Bound<'py, PyAny>)>> {
        let store = self.snapshot();
        let Some(flag) = store.get_flag(flag_key) else {
            return Ok(None);
        };
        let ctx = user_context(&flag, &store, user_key, attributes)?;
        let result = py.detach(|| evaluate_variant(&flag, &ctx, &store));
        Ok(Some((result.enabled, to_py(py, &result.value)?)))
    }

    /// The evaluation with its reason (`checkgate_core::evaluator::explain`)
    /// as a dict with the core's snake_case keys; absent optionals are
    /// omitted. None if the flag is unknown.
    #[pyo3(signature = (flag_key, user_key, attributes=None))]
    fn explain<'py>(
        &self,
        py: Python<'py>,
        flag_key: &str,
        user_key: &str,
        attributes: Option<&Bound<'py, PyDict>>,
    ) -> PyResult<Option<Bound<'py, PyAny>>> {
        let store = self.snapshot();
        let Some(flag) = store.get_flag(flag_key) else {
            return Ok(None);
        };
        let ctx = user_context(&flag, &store, user_key, attributes)?;
        let explanation = py.detach(|| explain(&flag, &ctx, &store));
        Ok(Some(to_py(py, &explanation)?))
    }

    /// Whether a flag with this key is held.
    fn contains(&self, key: &str) -> bool {
        self.snapshot().get_flag(key).is_some()
    }

    /// Number of flags held.
    fn flag_count(&self) -> usize {
        self.snapshot().len()
    }

    /// Keys of the flags held, sorted.
    fn flag_keys(&self) -> Vec<String> {
        let mut keys: Vec<String> = self
            .snapshot()
            .list_flags()
            .iter()
            .map(|f| f.key.clone())
            .collect();
        keys.sort_unstable();
        keys
    }

    fn __len__(&self) -> usize {
        self.flag_count()
    }
}

/// Checkgate's Rust evaluation engine (internal; use `checkgate.CheckgateClient`).
// The engine does not rely on the GIL for its own state (locks above), but
// abi3 wheels cannot load on free-threaded builds anyway; see README.
#[pymodule(gil_used = false)]
mod _core {
    #[pymodule_export]
    use super::Engine;

    #[pymodule_init]
    fn init(m: &pyo3::Bound<'_, pyo3::types::PyModule>) -> pyo3::PyResult<()> {
        use pyo3::types::PyModuleMethods;
        m.add("CORE_VERSION", env!("CARGO_PKG_VERSION"))
    }
}
