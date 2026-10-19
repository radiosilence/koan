package cc.blit.koan.ui.settings

import cc.blit.koan.ffi.Invite
import cc.blit.koan.ffi.KoanEngineInterface
import cc.blit.koan.ffi.Settings
import java.lang.reflect.Proxy
import kotlin.coroutines.Continuation
import kotlin.coroutines.intrinsics.COROUTINE_SUSPENDED

/**
 * A stand-in for the engine: each call is recorded by name with its
 * arguments, and answered from `answers`, else with `Unit`, so a call that
 * returns a value needs an answer. An answer that throws reaches a
 * suspending caller as the engine's own failure would.
 */
class FakeEngine {
    val calls = mutableListOf<Pair<String, List<Any?>>>()
    val answers = mutableMapOf<String, (List<Any?>) -> Any?>()

    fun called(name: String) = calls.filter { it.first == name }.map { it.second }

    val engine = Proxy.newProxyInstance(javaClass.classLoader, arrayOf(KoanEngineInterface::class.java)) { _, method, args ->
        val continuation = args?.lastOrNull() as? Continuation<*>
        val given = args.orEmpty().filterNot { it is Continuation<*> }
        // Kotlin mangles the names of functions taking or returning unsigned numbers.
        val name = method.name.substringBefore('-')
        calls += name to given
        val answer = runCatching { answers[name]?.invoke(given) ?: Unit.takeIf { name !in answers } }
        val failure = answer.exceptionOrNull()
        when {
            // The interface declares no exceptions, so a suspending call's
            // failure goes through its continuation rather than the proxy.
            failure != null && continuation != null -> {
                @Suppress("UNCHECKED_CAST")
                (continuation as Continuation<Any?>).resumeWith(Result.failure(failure))
                COROUTINE_SUSPENDED
            }
            failure != null -> throw failure
            else -> answer.getOrNull()
        }
    } as KoanEngineInterface
}

fun settings(signedIn: Boolean = true, url: String = "https://music.example.com", username: String = "droid") = Settings(
    libraryFolders = emptyList(),
    remoteEnabled = signedIn,
    remoteUrl = url,
    remoteUsername = username,
    remoteSignedIn = signedIn,
    remoteTracks = 3u,
    remoteLastSync = null,
    downloadWorkers = 4u,
    cacheLimit = "",
    downloadAhead = null,
    cacheDir = "",
    cacheBytes = 0u,
    autoSync = true,
    autoSyncIntervalMins = 60u,
    playQueue = false,
    replaygain = "off",
    preAmpDb = 0.0,
    fadeOnPause = true,
    devicesDiscoverable = true,
    devicesAddresses = emptyList(),
    devicesRefused = emptyList(),
    devicesNearbyControl = "full",
    devicesKeepRunning = false,
)

fun invite(server: String = "https://other.example.com", username: String = "guest") = Invite(
    server = server,
    username = username,
    token = "token",
    password = null,
    link = "https://koan.rocks/join/#server=$server&username=$username&invite=token",
    emailSubject = "",
    emailText = "",
    emailHtml = "",
    mailto = "",
)
