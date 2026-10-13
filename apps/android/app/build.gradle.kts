plugins {
    alias(libs.plugins.android.application)
    alias(libs.plugins.kotlin.android)
    alias(libs.plugins.kotlin.compose)
}

// The engine: koan-ffi built for Android, and the Kotlin bindings uniffi
// generates from it. Neither is checked in. This is `just android-ffi`, run
// by Gradle so that a build from a clean checkout has what it compiles
// against. `-PskipFfi` leaves what is already in jniLibs and the build
// directory alone, for a Kotlin-only edit loop.
val repoRoot: File = rootDir.resolve("../..").canonicalFile
val ndkVersion = "27.2.12479018"
val platformLevel = 28
val jniLibsDir: File = projectDir.resolve("src/main/jniLibs")
val bindingsDir: Provider<Directory> = layout.buildDirectory.dir("generated/uniffi/kotlin")
val skipFfi = providers.gradleProperty("skipFfi").isPresent

android {
    namespace = "cc.blit.koan"
    compileSdk = 36
    ndkVersion = this@Build_gradle.ndkVersion

    defaultConfig {
        applicationId = "cc.blit.koan"
        // AAudio's Usage and ContentType, which the engine links at load.
        minSdk = platformLevel
        targetSdk = 36
        versionCode = 1
        versionName = "0.0.0-phase2"
        // The ABIs the engine is built for: devices, and the emulator. No
        // 32-bit.
        ndk { abiFilters += listOf("arm64-v8a", "x86_64") }
    }

    buildTypes {
        debug {
            buildConfigField("boolean", "STRICT_MODE", "true")
        }
        release {
            isMinifyEnabled = true
            proguardFiles(getDefaultProguardFile("proguard-android-optimize.txt"), "proguard-rules.pro")
            buildConfigField("boolean", "STRICT_MODE", "false")
        }
    }

    buildFeatures {
        compose = true
        buildConfig = true
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    sourceSets.getByName("main").java.srcDir(bindingsDir)

    testOptions {
        unitTests.isReturnDefaultValues = true
    }
}

kotlin {
    compilerOptions {
        jvmTarget.set(org.jetbrains.kotlin.gradle.dsl.JvmTarget.JVM_17)
        allWarningsAsErrors.set(true)
    }
}

val sdkNdk = androidComponents.sdkComponents.ndkDirectory

val buildKoanLibs = tasks.register<Exec>("buildKoanLibs") {
    group = "koan"
    description = "cargo ndk: koan-ffi for arm64-v8a and x86_64 into src/main/jniLibs"
    workingDir = repoRoot
    onlyIf { !skipFfi }
    inputs.files(fileTree(repoRoot.resolve("crates")) {
        include("**/*.rs", "**/Cargo.toml", "**/build.rs", "**/uniffi.toml")
    })
    inputs.file(repoRoot.resolve("Cargo.lock"))
    outputs.dir(jniLibsDir)
    doFirst { environment("ANDROID_NDK_HOME", sdkNdk.get().asFile.absolutePath) }
    // `rustc --crate-type cdylib`: the crate also declares a staticlib for
    // Apple's linker, which Android has no use for and which doubles the link.
    commandLine(
        "cargo", "ndk", "-t", "arm64-v8a", "-t", "x86_64", "-P", platformLevel.toString(),
        "-o", jniLibsDir.absolutePath,
        "rustc", "--release", "-p", "koan-ffi", "--lib", "--crate-type", "cdylib",
    )
}

val generateKoanBindings = tasks.register<Exec>("generateKoanBindings") {
    group = "koan"
    description = "uniffi-bindgen: Kotlin bindings from the built library's metadata"
    workingDir = repoRoot
    dependsOn(buildKoanLibs)
    onlyIf { !skipFfi }
    inputs.dir(jniLibsDir.resolve("arm64-v8a")).optional()
    inputs.file(repoRoot.resolve("crates/koan-ffi/uniffi.toml"))
    outputs.dir(bindingsDir)
    doFirst { delete(bindingsDir) }
    commandLine(
        "cargo", "run", "-q", "-p", "uniffi-bindgen", "--", "generate",
        "--library", jniLibsDir.resolve("arm64-v8a/libkoan_ffi.so").absolutePath,
        "--language", "kotlin", "--out-dir", bindingsDir.get().asFile.absolutePath,
    )
}

// Bindings before any Kotlin compiles, the libraries before they are packaged.
tasks.configureEach {
    if (name.startsWith("compile") && name.contains("Kotlin")) dependsOn(generateKoanBindings)
    if (name.startsWith("merge") && name.endsWith("JniLibFolders")) dependsOn(buildKoanLibs)
    if (name.startsWith("ksp") || name.startsWith("lint")) dependsOn(generateKoanBindings)
}

// androidx.core 1.18+ needs compileSdk 37 and AGP 9; AGP 8.13 tops out at 36.
// Hold it where this toolchain can build it, whoever asks for newer.
configurations.configureEach {
    resolutionStrategy.force("androidx.core:core:${libs.versions.coreKtx.get()}")
}

dependencies {
    implementation(libs.kotlinx.coroutines.android)
    implementation(platform(libs.androidx.compose.bom))
    implementation(libs.androidx.compose.ui)
    implementation(libs.androidx.compose.foundation)
    implementation(libs.androidx.compose.material3)
    implementation(libs.androidx.activity.compose)
    implementation(libs.androidx.core.ktx)
    implementation(libs.androidx.lifecycle.runtime.compose)
    implementation(libs.androidx.media3.session)
    implementation(libs.androidx.media3.common)
    // uniffi's generated Kotlin crosses into the engine through JNA; on
    // Android that is the AAR, which carries libjnidispatch.
    implementation("${libs.jna.get()}@aar")
    // rustls-platform-verifier's Kotlin half, which the engine's TLS calls
    // back into.
    implementation(libs.rustls.platform.verifier)

    testImplementation(libs.junit)
    testImplementation(libs.kotlinx.coroutines.test)
    // The JVM jar, so the generated bindings load in a unit test.
    testImplementation(libs.jna)
}
