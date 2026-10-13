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
        // The Kotlin half of rustls-platform-verifier, which the core's HTTPS
        // verifies certificates through. It ships inside the crate rather than
        // on a Maven server, so the repository is wherever cargo unpacked it.
        maven {
            url = uri(rustlsPlatformVerifierMaven())
            metadataSources.artifact()
        }
    }
}

fun rustlsPlatformVerifierMaven(): File {
    val metadata = providers.exec {
        workingDir = rootDir.resolve("../..")
        commandLine(
            "cargo", "metadata", "--format-version", "1",
            "--filter-platform", "aarch64-linux-android",
            "--manifest-path", "crates/koan-ffi/Cargo.toml",
        )
    }.standardOutput.asText.get()
    @Suppress("UNCHECKED_CAST")
    val packages = (groovy.json.JsonSlurper().parseText(metadata) as Map<String, Any>)["packages"] as List<Map<String, Any>>
    val manifest = packages.first { it["name"] == "rustls-platform-verifier-android" }["manifest_path"] as String
    return File(manifest).parentFile.resolve("maven")
}

rootProject.name = "koan"
include(":app")
