package cc.blit.koan

import android.content.Context

/**
 * The one JNI entry point of the engine library. Everything else crosses by
 * JNA through the generated `uniffi.koan_ffi` bindings; this cannot, because
 * it needs a `JNIEnv`.
 *
 * `initTls` gives rustls-platform-verifier the application context, so that
 * HTTPS can check certificates against the system's trust store. Without it
 * every request the engine makes fails. It must run once, before the engine
 * is created, with the *application* context (an activity's would be leaked
 * by the verifier). The Rust side is `Java_cc_blit_koan_Native_initTls` in
 * `crates/koan-ffi/src/android.rs`, so this object's name and package are not
 * free to change.
 */
object Native {
    init {
        System.loadLibrary("koan_ffi")
    }

    private external fun initTls(context: Context)

    @Volatile
    private var tlsReady = false

    /** Idempotent. Throws `RuntimeException` if the verifier could not be set up. */
    @Synchronized
    fun initTlsOnce(context: Context) {
        if (tlsReady) return
        initTls(context.applicationContext)
        tlsReady = true
    }
}
