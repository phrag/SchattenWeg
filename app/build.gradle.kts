import java.util.Properties

plugins {
    alias(libs.plugins.android.application)
    alias(libs.plugins.kotlin.android)
    alias(libs.plugins.kotlin.compose)
}

// Release signing credentials come from an UNTRACKED keystore.properties in
// the repo root, or from environment variables in CI. Never from git — both
// the keystore and this properties file are gitignored, and the build simply
// produces an unsigned release when they are absent.
val keystoreProperties = Properties().apply {
    val f = rootProject.file("keystore.properties")
    if (f.exists()) f.inputStream().use { load(it) }
}

fun signingValue(key: String, env: String): String? =
    keystoreProperties.getProperty(key) ?: System.getenv(env)

val releaseStoreFile = signingValue("storeFile", "SCHATTENWEG_STORE_FILE")
val releaseStorePassword = signingValue("storePassword", "SCHATTENWEG_STORE_PASSWORD")
val releaseKeyAlias = signingValue("keyAlias", "SCHATTENWEG_KEY_ALIAS")
val releaseKeyPassword = signingValue("keyPassword", "SCHATTENWEG_KEY_PASSWORD")
val hasReleaseSigning = listOf(
    releaseStoreFile,
    releaseStorePassword,
    releaseKeyAlias,
    releaseKeyPassword,
).all { !it.isNullOrBlank() }

android {
    namespace = "de.schattenweg.app"
    compileSdk = 36

    defaultConfig {
        applicationId = "de.schattenweg.app"
        minSdk = 29
        targetSdk = 36
        versionCode = 1
        versionName = "0.1.0"
    }

    signingConfigs {
        if (hasReleaseSigning) {
            create("release") {
                storeFile = file(releaseStoreFile!!)
                storePassword = releaseStorePassword
                keyAlias = releaseKeyAlias
                keyPassword = releaseKeyPassword
                enableV3Signing = true
                enableV4Signing = true
            }
        }
    }

    buildTypes {
        release {
            isMinifyEnabled = true
            isShrinkResources = true
            isDebuggable = false
            proguardFiles(
                getDefaultProguardFile("proguard-android-optimize.txt"),
                "proguard-rules.pro",
            )
            signingConfig = if (hasReleaseSigning) {
                signingConfigs.getByName("release")
            } else {
                // Deliberately NOT the debug key: an APK signed with the
                // public debug key would look signed while being trivially
                // forgeable. Unsigned is the honest failure mode.
                null
            }
        }
        debug {
            applicationIdSuffix = ".debug"
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    buildFeatures {
        compose = true
    }
}

kotlin {
    jvmToolchain(17)
}

dependencies {
    implementation(platform(libs.compose.bom))
    implementation(libs.compose.ui)
    implementation(libs.compose.material3)
    implementation(libs.activity.compose)
    implementation(libs.lifecycle.viewmodel.compose)
    implementation(libs.maplibre)
    // UniFFI-generated bindings load the Rust core through JNA (Android aar).
    implementation(libs.jna) {
        artifact {
            type = "aar"
        }
    }
}

// ---------------------------------------------------------------------------
// Rust core integration. Three steps, all wired into preBuild:
//   1. host-build the core (cdylib) so uniffi-bindgen can read its metadata
//   2. generate the Kotlin bindings into src/main/java (gitignored)
//   3. cross-compile the core for Android ABIs into src/main/jniLibs
// Requires: rustup targets aarch64-linux-android/x86_64-linux-android and
// cargo-ndk (cargo install cargo-ndk), plus ANDROID_NDK_HOME.
// ---------------------------------------------------------------------------

val rustDir = rootProject.file("core")

val cargoHostBuild by tasks.registering(Exec::class) {
    group = "rust"
    description = "Build the Rust core for the host (input to uniffi-bindgen)."
    workingDir = rustDir
    commandLine("cargo", "build", "--release", "--lib")
}

val generateUniffiBindings by tasks.registering(Exec::class) {
    group = "rust"
    description = "Generate Kotlin bindings from the compiled Rust core."
    dependsOn(cargoHostBuild)
    workingDir = rustDir
    val libName =
        if (System.getProperty("os.name").lowercase().contains("mac")) {
            "libschattenweg_core.dylib"
        } else {
            "libschattenweg_core.so"
        }
    commandLine(
        "cargo", "run", "--release", "--bin", "uniffi-bindgen", "--",
        "generate",
        "--library", "target/release/$libName",
        "--language", "kotlin",
        "--out-dir", layout.projectDirectory.dir("src/main/java").asFile.absolutePath,
    )
}

val cargoNdkBuild by tasks.registering(Exec::class) {
    group = "rust"
    description = "Cross-compile the Rust core for Android ABIs."
    workingDir = rustDir
    commandLine(
        "cargo", "ndk",
        "-t", "arm64-v8a",
        "-t", "x86_64",
        "-o", layout.projectDirectory.dir("src/main/jniLibs").asFile.absolutePath,
        "build", "--release",
    )
}

tasks.named("preBuild") {
    dependsOn(generateUniffiBindings, cargoNdkBuild)
}
