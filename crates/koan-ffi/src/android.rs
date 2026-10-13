//! What the core needs from the JVM on Android, which uniffi cannot carry.

use jni::objects::{JClass, JObject};

/// Hand the core the app's `Context`. reqwest verifies every HTTPS
/// certificate through rustls-platform-verifier, which asks Android's trust
/// store, user-installed authorities included, and panics without a context.
///
/// Called once by `KoanApp`, with the application context, before the engine
/// opens.
#[unsafe(no_mangle)]
pub extern "system" fn Java_cc_blit_koan_KoanApp_attachContext<'local>(
    mut env: jni::EnvUnowned<'local>,
    _class: JClass<'local>,
    context: JObject<'local>,
) {
    let outcome = env
        .with_env(|env| rustls_platform_verifier::android::init_with_env(env, context))
        .into_outcome();
    if !matches!(outcome, jni::Outcome::Ok(())) {
        log::error!("android: certificate verification not initialised; HTTPS will fail");
    }
}
