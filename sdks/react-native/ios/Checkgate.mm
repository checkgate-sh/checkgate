#import <React/RCTBridge+Private.h>
#import <React/RCTBridgeModule.h>
#import <jsi/jsi.h>

#import "checkgate_jsi.h"

// Installs the Checkgate JSI host object (`global.__CheckgateInternal`, backed
// by the Rust evaluation engine) on the JS runtime. The JS client calls
// `install()` synchronously the first time it is constructed.
@interface Checkgate : NSObject <RCTBridgeModule>
@end

@implementation Checkgate

@synthesize bridge = _bridge;

RCT_EXPORT_MODULE(Checkgate)

+ (BOOL)requiresMainQueueSetup
{
  return NO;
}

// Blocking synchronous methods run on the JS thread, so touching the runtime
// here is safe.
RCT_EXPORT_BLOCKING_SYNCHRONOUS_METHOD(install)
{
  // RCTCxxBridge (old architecture) and RCTBridgeProxy (bridgeless) both expose
  // the JSI runtime as `runtime`.
  RCTCxxBridge *bridge = (RCTCxxBridge *)self.bridge;
  if (bridge == nil || ![bridge respondsToSelector:@selector(runtime)]) {
    return @NO;
  }
  auto *runtime = static_cast<facebook::jsi::Runtime *>(bridge.runtime);
  if (runtime == nullptr) {
    return @NO;
  }
  checkgate::installCheckgateJSI(*runtime);
  return @YES;
}

@end
