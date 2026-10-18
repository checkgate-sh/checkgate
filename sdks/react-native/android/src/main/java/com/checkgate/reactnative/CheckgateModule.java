package com.checkgate.reactnative;

import androidx.annotation.NonNull;

import com.facebook.react.bridge.JavaScriptContextHolder;
import com.facebook.react.bridge.ReactApplicationContext;
import com.facebook.react.bridge.ReactContextBaseJavaModule;
import com.facebook.react.bridge.ReactMethod;
import com.facebook.react.module.annotations.ReactModule;

/**
 * Installs the Checkgate JSI host object ({@code global.__CheckgateInternal},
 * backed by the Rust evaluation engine) on the JS runtime. The JS client calls
 * {@link #install()} synchronously the first time it is constructed.
 */
@ReactModule(name = CheckgateModule.NAME)
public class CheckgateModule extends ReactContextBaseJavaModule {
    public static final String NAME = "Checkgate";

    static {
        // libcheckgate.so: the JSI host (cpp/) with the Rust engine linked in.
        System.loadLibrary("checkgate");
    }

    private static native void nativeInstall(long jsiRuntimePointer);

    CheckgateModule(ReactApplicationContext context) {
        super(context);
    }

    @NonNull
    @Override
    public String getName() {
        return NAME;
    }

    /** Runs on the JS thread, so touching the runtime here is safe. */
    @ReactMethod(isBlockingSynchronousMethod = true)
    public boolean install() {
        JavaScriptContextHolder holder = getReactApplicationContext().getJavaScriptContextHolder();
        if (holder == null || holder.get() == 0) {
            return false;
        }
        nativeInstall(holder.get());
        return true;
    }
}
