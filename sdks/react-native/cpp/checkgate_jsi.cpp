#include "checkgate_jsi.h"
#include "checkgate_core.h"
#include <jsi/jsi.h>
#include <memory>
#include <string>

using namespace facebook::jsi;
using namespace std;

namespace checkgate {

// ---------------------------------------------------------------------------
// Helper: call JSON.stringify on a JSI Value and return a std::string.
// Used to serialise JS arrays/objects (rules, attributes) before crossing
// the FFI boundary into Rust.
// ---------------------------------------------------------------------------
static string jsi_to_json(Runtime &runtime, const Value &val) {
  auto jsonStringify = runtime.global()
                           .getPropertyAsObject(runtime, "JSON")
                           .getPropertyAsFunction(runtime, "stringify");
  Value result = jsonStringify.call(runtime, val);
  if (result.isString()) {
    return result.getString(runtime).utf8(runtime);
  }
  return "null";
}

// ---------------------------------------------------------------------------
// installCheckgateJSI
//
// Installs the __CheckgateInternal global on the JS runtime with these methods:
//   upsertFlag(key, isEnabled, rolloutPct, rulesArray)   [legacy, boolean flags]
//   upsertFlagV2(flagObject)                             [multi-variant flags]
//   deleteFlag(key)
//   clearStore()
//   isEnabled(flagKey, userKey, attributesObject) -> bool
//   getVariant(flagKey, userKey, attributesObject) -> string (JSON)
// ---------------------------------------------------------------------------
void installCheckgateJSI(Runtime &jsiRuntime) {

  // -- upsertFlag -----------------------------------------------------------
  auto upsertFlag = Function::createFromHostFunction(
      jsiRuntime, PropNameID::forAscii(jsiRuntime, "upsertFlag"), 4,
      [](Runtime &runtime, const Value & /*thisValue*/, const Value *arguments,
         size_t count) -> Value {
        if (count < 2 || !arguments[0].isString()) {
          throw JSError(runtime, "[Checkgate] upsertFlag: expected (key: string, isEnabled: bool, rolloutPct?: number, rules?: array)");
        }

        string key = arguments[0].getString(runtime).utf8(runtime);
        bool isEnabled = count > 1 && arguments[1].isBool() ? arguments[1].getBool() : false;

        // rolloutPercentage: number | null | undefined → -1 means "no cap"
        int rollout = -1;
        if (count > 2 && arguments[2].isNumber()) {
          rollout = static_cast<int>(arguments[2].getNumber());
        }

        // rules: array of targeting-rule objects → JSON string for Rust
        string rulesJson = "[]";
        if (count > 3 && !arguments[3].isNull() && !arguments[3].isUndefined()) {
          rulesJson = jsi_to_json(runtime, arguments[3]);
        }

        checkgate_upsert_flag(key.c_str(), isEnabled, rollout, rulesJson.c_str());

        return Value::undefined();
      });

  // -- upsertFlagV2 ---------------------------------------------------------
  // Accepts the full flag object (flag_type, default/disabled values, per-rule
  // variants) so multi-variant flags evaluate correctly via getVariant.
  auto upsertFlagV2 = Function::createFromHostFunction(
      jsiRuntime, PropNameID::forAscii(jsiRuntime, "upsertFlagV2"), 1,
      [](Runtime &runtime, const Value & /*thisValue*/, const Value *arguments,
         size_t count) -> Value {
        if (count < 1 || !arguments[0].isObject()) {
          throw JSError(runtime, "[Checkgate] upsertFlagV2: expected (flag: object)");
        }
        string flagJson = jsi_to_json(runtime, arguments[0]);
        checkgate_upsert_flag_v2(flagJson.c_str());
        return Value::undefined();
      });

  // -- deleteFlag -----------------------------------------------------------
  auto deleteFlag = Function::createFromHostFunction(
      jsiRuntime, PropNameID::forAscii(jsiRuntime, "deleteFlag"), 1,
      [](Runtime &runtime, const Value & /*thisValue*/, const Value *arguments,
         size_t count) -> Value {
        if (count < 1 || !arguments[0].isString()) {
          throw JSError(runtime, "[Checkgate] deleteFlag: expected (key: string)");
        }
        string key = arguments[0].getString(runtime).utf8(runtime);
        checkgate_delete_flag(key.c_str());
        return Value::undefined();
      });

  // -- clearStore -----------------------------------------------------------
  auto clearStore = Function::createFromHostFunction(
      jsiRuntime, PropNameID::forAscii(jsiRuntime, "clearStore"), 0,
      [](Runtime & /*runtime*/, const Value & /*thisValue*/,
         const Value * /*arguments*/, size_t /*count*/) -> Value {
        checkgate_clear_store();
        return Value::undefined();
      });

  // -- isEnabled ------------------------------------------------------------
  // Synchronous, zero-network call: JS → C++ → Rust DashMap → bool.
  auto isEnabled = Function::createFromHostFunction(
      jsiRuntime, PropNameID::forAscii(jsiRuntime, "isEnabled"), 3,
      [](Runtime &runtime, const Value & /*thisValue*/, const Value *arguments,
         size_t count) -> Value {
        if (count < 2 || !arguments[0].isString() || !arguments[1].isString()) {
          throw JSError(runtime, "[Checkgate] isEnabled: expected (flagKey: string, userKey: string, attributes?: object)");
        }

        string flagKey = arguments[0].getString(runtime).utf8(runtime);
        string userKey = arguments[1].getString(runtime).utf8(runtime);

        // attributes: plain JS object { [key: string]: string }
        string attrsJson = "{}";
        if (count > 2 && !arguments[2].isNull() && !arguments[2].isUndefined()) {
          attrsJson = jsi_to_json(runtime, arguments[2]);
        }

        int result = checkgate_is_enabled(
            flagKey.c_str(), userKey.c_str(), attrsJson.c_str());

        return Value(result != 0);
      });

  // -- getVariant -----------------------------------------------------------
  // Returns the full evaluation `{enabled, value}` as a JSON string; the JS
  // wrapper JSON.parses it. Returns the string "null" if the flag is missing.
  auto getVariant = Function::createFromHostFunction(
      jsiRuntime, PropNameID::forAscii(jsiRuntime, "getVariant"), 3,
      [](Runtime &runtime, const Value & /*thisValue*/, const Value *arguments,
         size_t count) -> Value {
        if (count < 2 || !arguments[0].isString() || !arguments[1].isString()) {
          throw JSError(runtime, "[Checkgate] getVariant: expected (flagKey: string, userKey: string, attributes?: object)");
        }

        string flagKey = arguments[0].getString(runtime).utf8(runtime);
        string userKey = arguments[1].getString(runtime).utf8(runtime);

        string attrsJson = "{}";
        if (count > 2 && !arguments[2].isNull() && !arguments[2].isUndefined()) {
          attrsJson = jsi_to_json(runtime, arguments[2]);
        }

        char *raw = checkgate_get_variant(
            flagKey.c_str(), userKey.c_str(), attrsJson.c_str());
        string json = raw ? string(raw) : "null";
        checkgate_free_string(raw);

        return String::createFromUtf8(runtime, json);
      });

  // -- Bind to global.__CheckgateInternal -----------------------------------
  Object checkgateModule = Object(jsiRuntime);
  checkgateModule.setProperty(jsiRuntime, "upsertFlag",   std::move(upsertFlag));
  checkgateModule.setProperty(jsiRuntime, "upsertFlagV2", std::move(upsertFlagV2));
  checkgateModule.setProperty(jsiRuntime, "deleteFlag",   std::move(deleteFlag));
  checkgateModule.setProperty(jsiRuntime, "clearStore",   std::move(clearStore));
  checkgateModule.setProperty(jsiRuntime, "isEnabled",    std::move(isEnabled));
  checkgateModule.setProperty(jsiRuntime, "getVariant",   std::move(getVariant));

  jsiRuntime.global().setProperty(jsiRuntime, "__CheckgateInternal",
                                  std::move(checkgateModule));
}

} // namespace checkgate
