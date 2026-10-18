// JNI entry point for CheckgateModule.nativeInstall(): hands the JS runtime
// that React Native owns to installCheckgateJSI().
#include <jni.h>
#include <jsi/jsi.h>

#include "checkgate_jsi.h"

extern "C" JNIEXPORT void JNICALL
Java_com_checkgate_reactnative_CheckgateModule_nativeInstall(JNIEnv * /*env*/,
                                                             jclass /*clazz*/,
                                                             jlong jsiRuntimePointer) {
  auto *runtime = reinterpret_cast<facebook::jsi::Runtime *>(jsiRuntimePointer);
  if (runtime != nullptr) {
    checkgate::installCheckgateJSI(*runtime);
  }
}
