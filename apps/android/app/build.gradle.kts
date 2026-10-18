import javax.inject.Inject

plugins {
    alias(libs.plugins.android.application)
    alias(libs.plugins.kotlin.compose)
    alias(libs.plugins.kotlin.serialization)
}

/** The commit built, short; "unknown" outside a checkout. */
val commit = providers.exec {
    commandLine("git", "rev-parse", "--short=8", "HEAD")
    isIgnoreExitValue = true
}.standardOutput.asText.map { it.trim().ifEmpty { "unknown" } }

/**
 * The Kotlin half of rustls-platform-verifier must be the version of the
 * `rustls-platform-verifier-android` crate the core is built with, so it is
 * read from Cargo.lock rather than pinned twice.
 */
val rustlsPlatformVerifier = providers.fileContents(rootProject.layout.projectDirectory.file("../../Cargo.lock"))
    .asText.map { lock ->
        val lines = lock.lines()
        val at = lines.indexOfFirst { it.trim() == "name = \"rustls-platform-verifier-android\"" }
        check(at >= 0) { "rustls-platform-verifier-android is not in Cargo.lock" }
        lines[at + 1].substringAfter('"').substringBefore('"')
    }.get()

/** The ABIs the core is built for. `-Pabis=arm64-v8a` builds one, for a device or an emulator on Apple silicon. */
val koanAbis = providers.gradleProperty("abis").orElse("arm64-v8a,x86_64").map { it.split(",") }

android {
    namespace = "cc.blit.koan"
    compileSdk = 37
    ndkVersion = "30.0.16248370"

    defaultConfig {
        applicationId = "cc.blit.koan"
        minSdk = 29
        targetSdk = 37
        versionCode = 1
        versionName = "0.64.1"
        // Which build a tester's log came from.
        buildConfigField("String", "COMMIT", "\"${commit.get()}\"")
        // Only what the core is built for: a device would otherwise pick a
        // 32-bit ABI that JNA ships and koan does not, and fail to load.
        ndk { abiFilters += koanAbis.get() }
    }

    // The release key when the build is given it, as CI is: every APK it
    // builds is then an update to the last. Without it, this machine's debug
    // key, so a local build still sideloads. The key and its password live in
    // 1Password and in the repository's secrets, never here.
    val releaseKeystore = providers.environmentVariable("KOAN_ANDROID_KEYSTORE").orNull
    signingConfigs {
        if (releaseKeystore != null) {
            create("release") {
                storeFile = file(releaseKeystore)
                storeType = "pkcs12"
                storePassword = providers.environmentVariable("KOAN_ANDROID_KEYSTORE_PASSWORD").get()
                keyAlias = "koan"
                keyPassword = storePassword
            }
        }
    }

    buildTypes {
        release {
            signingConfig = signingConfigs.findByName("release") ?: signingConfigs.getByName("debug")
            isMinifyEnabled = true
            isShrinkResources = true
            proguardFiles(getDefaultProguardFile("proguard-android-optimize.txt"), "proguard-rules.pro")
        }
    }

    buildFeatures {
        compose = true
        buildConfig = true
    }

    // Lint cannot read the ABIs through the `abis` property, so it reports
    // x86_64 missing even in the default build, which carries it.
    lint {
        disable += "ChromeOsAbiSupport"
    }

    // Robolectric renders composables in unit tests, which needs the app's resources.
    testOptions {
        unitTests.isIncludeAndroidResources = true
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_21
        targetCompatibility = JavaVersion.VERSION_21
    }
}

kotlin {
    jvmToolchain(21)
}

/**
 * koan-core, built with cargo-ndk for each ABI into `jniLibs`, and the Kotlin
 * bindings uniffi generates from the built library.
 *
 * Always a release build of the core: a debug one decodes too slowly to keep
 * the audio fed.
 */
abstract class KoanFfi @Inject constructor(private val exec: ExecOperations) : DefaultTask() {
    @get:Input
    abstract val abis: ListProperty<String>

    @get:Input
    abstract val minSdk: Property<Int>

    @get:Internal
    abstract val workspace: DirectoryProperty

    @get:Internal
    abstract val ndk: DirectoryProperty

    @get:OutputDirectory
    abstract val jniLibs: DirectoryProperty

    @get:OutputDirectory
    abstract val kotlin: DirectoryProperty

    @TaskAction
    fun build() {
        val root = workspace.get().asFile
        exec.exec {
            workingDir = root
            environment("ANDROID_NDK_HOME", ndk.get().asFile.path)
            commandLine(
                listOf("cargo", "ndk", "--platform", minSdk.get().toString(), "-o", jniLibs.get().asFile.path) +
                    abis.get().flatMap { listOf("-t", it) } +
                    listOf("build", "--release", "-p", "koan-ffi"),
            )
        }
        val lib = jniLibs.get().dir(abis.get().first()).file("libkoan_ffi.so").asFile
        kotlin.get().asFile.deleteRecursively()
        exec.exec {
            workingDir = root
            commandLine(
                "cargo", "run", "-q", "-p", "uniffi-bindgen", "--",
                "generate", "--library", lib.path, "--language", "kotlin", "--no-format",
                "--out-dir", kotlin.get().asFile.path,
            )
        }
    }
}

val koanFfi = tasks.register<KoanFfi>("koanFfi") {
    abis.set(koanAbis)
    minSdk.set(android.defaultConfig.minSdk!!)
    workspace.set(rootProject.layout.projectDirectory.dir("../.."))
    ndk.set(androidComponents.sdkComponents.ndkDirectory)
    jniLibs.set(layout.buildDirectory.dir("koan-ffi/jniLibs"))
    kotlin.set(layout.buildDirectory.dir("koan-ffi/kotlin"))
    // cargo knows what changed; Gradle cannot see the Rust sources.
    outputs.upToDateWhen { false }
}

androidComponents {
    onVariants { variant ->
        variant.sources.jniLibs?.addGeneratedSourceDirectory(koanFfi, KoanFfi::jniLibs)
        variant.sources.kotlin?.addGeneratedSourceDirectory(koanFfi, KoanFfi::kotlin)
    }
}

dependencies {
    implementation(libs.androidx.core.ktx)
    implementation(libs.androidx.activity.compose)
    implementation(libs.androidx.lifecycle.runtime.compose)
    implementation(libs.androidx.lifecycle.viewmodel.compose)
    implementation(libs.kotlinx.coroutines.android)
    implementation(libs.media3.session)
    implementation(libs.androidx.navigation3.runtime)
    implementation(libs.androidx.navigation3.ui)
    implementation(libs.androidx.lifecycle.viewmodel.navigation3)
    implementation(libs.kotlinx.serialization.core)
    implementation(libs.coil.compose)
    implementation(libs.jna) { artifact { type = "aar" } }
    implementation("org.rustls:rustls-platform-verifier:$rustlsPlatformVerifier")
    implementation(platform(libs.compose.bom))
    implementation(libs.compose.ui)
    implementation(libs.compose.ui.tooling.preview)
    implementation(libs.compose.material3)
    implementation(libs.reorderable)
    debugImplementation(libs.compose.ui.tooling)
    debugImplementation(libs.compose.ui.test.manifest)
    testImplementation(libs.junit)
    testImplementation(libs.kotlinx.coroutines.test)
    testImplementation(libs.robolectric)
    testImplementation(platform(libs.compose.bom))
    testImplementation(libs.compose.ui.test.junit4)
}
