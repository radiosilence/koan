//! What only an Android app needs from the library, apart from the engine.
//!
//! The engine itself is reached through the generated Kotlin bindings, which
//! cross by JNA. Two things cannot: they need a `JNIEnv`, and uniffi has none
//! to give.

use jni::EnvUnowned;
use jni::objects::{JClass, JObject};

/// Give `rustls-platform-verifier` the application context, so that HTTPS
/// can check certificates against the system's trust store. Without it every
/// request koan makes fails.
///
/// The Kotlin side declares and calls it, once, before anything else in the
/// engine, with the application context (not an activity's):
///
/// ```kotlin
/// package cc.blit.koan
///
/// object Native {
///     init { System.loadLibrary("koan_ffi") }
///     external fun initTls(context: android.content.Context)
/// }
/// ```
///
/// The verifier's own Kotlin component (`rustls:rustls-platform-verifier`)
/// must be on the app's classpath: it is what the Rust side calls back into.
/// An error is thrown into Kotlin as a `RuntimeException`.
#[unsafe(no_mangle)]
pub extern "system" fn Java_cc_blit_koan_Native_initTls<'local>(
    mut env: EnvUnowned<'local>,
    _class: JClass<'local>,
    context: JObject<'local>,
) {
    env.with_env(|env| rustls_platform_verifier::android::init_with_env(env, context))
        .resolve::<jni::errors::ThrowRuntimeExAndDefault>()
}

/// Send a warning or error to logcat, as well as to the log file: the file is
/// in the app's private directory, and logcat is what `adb` can see.
pub(crate) fn to_logcat(record: &log::Record) {
    if record.level() <= log::Level::Warn {
        android_logger::log(record);
    }
}
