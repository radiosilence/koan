//! What the core needs from the JVM on Android, which uniffi cannot carry.

use std::ffi::c_void;

use jni_sys::{JNIEnv, JavaVM, jclass, jobject};

/// Hand the core the app's `Context`, for the Android APIs it reaches over
/// JNI, each of which panics without one: cpal asks `AudioManager` how many
/// bursts the mixer buffers when it opens a stream, and reqwest verifies every
/// HTTPS certificate through the platform's trust store, user-installed
/// authorities included.
///
/// Called once by `KoanApp` before the engine opens, with the application
/// context, which lives as long as the process; the global reference taken
/// here is never released.
///
/// # Safety
///
/// Called by the JVM with a valid environment and a live `Context`.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn Java_cc_blit_koan_KoanApp_attachContext(
    env: *mut JNIEnv,
    _class: jclass,
    context: jobject,
) {
    // SAFETY: the JVM passes a valid environment, whose function table is
    // populated, and a live local reference to the context.
    unsafe {
        let functions = &**env;
        let mut vm: *mut JavaVM = std::ptr::null_mut();
        if (functions.v1_1.GetJavaVM)(env, &mut vm) != 0 {
            log::error!("android: no JavaVM to hand the core");
            return;
        }
        let global = (functions.v1_1.NewGlobalRef)(env, context);
        ndk_context::initialize_android_context(vm.cast::<c_void>(), global.cast::<c_void>());

        let mut unowned = jni::EnvUnowned::from_raw(env);
        let outcome = unowned
            .with_env(|env| {
                let context = jni::objects::JObject::from_raw(env, context);
                rustls_platform_verifier::android::init_with_env(env, context)
            })
            .into_outcome();
        if !matches!(outcome, jni::Outcome::Ok(())) {
            log::error!("android: certificate verification not initialised; HTTPS will fail");
        }
    }
}
