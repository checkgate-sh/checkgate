#pragma once

#include <jsi/jsi.h>

namespace checkgate {

// Installs `global.__CheckgateInternal` — the Rust evaluation engine — on the
// given JS runtime. Must be called on the JS thread. The platform modules
// (android/…/CheckgateModule.java, ios/Checkgate.mm) call it when JS invokes
// `NativeModules.Checkgate.install()`.
void installCheckgateJSI(facebook::jsi::Runtime &jsiRuntime);

} // namespace checkgate
