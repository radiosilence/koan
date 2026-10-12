import groovy.json.JsonSlurper
import java.io.File

pluginManagement {
    repositories {
        google()
        mavenCentral()
        gradlePluginPortal()
    }
}

dependencyResolutionManagement {
    repositoriesMode.set(RepositoriesMode.FAIL_ON_PROJECT_REPOS)
    repositories {
        google()
        mavenCentral()
        // The JVM half of rustls-platform-verifier is not published anywhere:
        // the `rustls-platform-verifier-android` crate carries it as a Maven
        // repository in its own `maven` directory. Ask cargo where that crate
        // is, so the version Cargo.lock pins is the one the app compiles
        // against. Without it every HTTPS request from the engine fails.
        maven {
            url = uri(platformVerifierMaven())
            content { includeGroup("rustls") }
        }
    }
}

rootProject.name = "koan-android"
include(":app")

fun platformVerifierMaven(): File {
    val manifest = File(settingsDir, "../../Cargo.toml").canonicalFile
    val process = ProcessBuilder(
        "cargo", "metadata", "--format-version", "1",
        "--filter-platform", "aarch64-linux-android",
        "--manifest-path", manifest.path,
    ).redirectError(ProcessBuilder.Redirect.DISCARD).start()
    val json = process.inputStream.bufferedReader().readText()
    check(process.waitFor() == 0) {
        "`cargo metadata` failed: the Android build needs cargo on PATH, to find rustls-platform-verifier's Maven repository"
    }
    @Suppress("UNCHECKED_CAST")
    val packages = (JsonSlurper().parseText(json) as Map<String, Any?>)["packages"] as List<Map<String, Any?>>
    val crate = packages.first { it["name"] == "rustls-platform-verifier-android" }
    return File(File(crate["manifest_path"] as String).parentFile, "maven")
}
