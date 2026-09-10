import org.gradle.internal.os.OperatingSystem

plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
    id("org.jetbrains.kotlin.plugin.compose")
}

android {
    namespace = "com.mykvm.receiver"
    compileSdk = 35
    // Pinned so the linker/stripper version cannot drift from the one the Rust
    // build script resolves out of the NDK directory.
    ndkVersion = "27.2.12479018"

    defaultConfig {
        applicationId = "com.mykvm.receiver"
        // Android 11: the first release where Shizuku can be started from the
        // phone itself (wireless debugging), so the whole setup works with no
        // PC and no root.
        minSdk = 30
        targetSdk = 35
        versionCode = 1
        versionName = "0.1.0"

        ndk {
            // arm64 covers every current phone; x86_64 lets the app run on the
            // emulator for development. Override with -PrustAbis=arm64-v8a.
            abiFilters += (providers.gradleProperty("rustAbis").orNull
                ?: "arm64-v8a,x86_64").split(",")
        }
    }

    buildTypes {
        release {
            // The protocol core is reached only through JNI, so the keep rules in
            // proguard-rules.pro are load-bearing here.
            isMinifyEnabled = true
            isShrinkResources = true
            proguardFiles(
                getDefaultProguardFile("proguard-android-optimize.txt"),
                "proguard-rules.pro",
            )
        }
        debug {
            isMinifyEnabled = false
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    kotlinOptions {
        jvmTarget = "17"
    }

    buildFeatures {
        compose = true
        buildConfig = true
    }

    packaging {
        jniLibs {
            useLegacyPackaging = false
        }
    }
}

dependencies {
    implementation("androidx.core:core-ktx:1.13.1")
    implementation("androidx.activity:activity-compose:1.9.3")
    implementation("androidx.lifecycle:lifecycle-runtime-ktx:2.8.7")
    implementation("androidx.lifecycle:lifecycle-service:2.8.7")

    val composeBom = platform("androidx.compose:compose-bom:2024.10.01")
    implementation(composeBom)
    implementation("androidx.compose.ui:ui")
    implementation("androidx.compose.ui:ui-tooling-preview")
    implementation("androidx.compose.material3:material3")
    implementation("androidx.compose.material:material-icons-extended")
    debugImplementation("androidx.compose.ui:ui-tooling")

    // Shizuku: lets this app run with the `shell` user's INJECT_EVENTS
    // permission, which is the only way to inject KeyEvents without root.
    implementation("dev.rikka.shizuku:api:13.1.5")
    implementation("dev.rikka.shizuku:provider:13.1.5")
}

// ---------------------------------------------------------------------------
// Rust core
// ---------------------------------------------------------------------------

val rustDir = rootProject.layout.projectDirectory.dir("rust").asFile
val jniLibsDir = layout.projectDirectory.dir("src/main/jniLibs").asFile

/** Android API level the shared libraries are linked against (== minSdk). */
val rustApiLevel = 30

/** Maps an ABI to its Rust target triple; also the set of ABIs we build. */
val abiToTriple = mapOf(
    "arm64-v8a" to "aarch64-linux-android",
    "x86_64" to "x86_64-linux-android",
)

/**
 * Builds `libmykvm_core.so` for every ABI in the NDK `abiFilters` and copies it
 * into `src/main/jniLibs/<abi>/` so the Android plugin packages it.
 *
 * The NDK comes from `ndk.dir` in local.properties or ANDROID_NDK_HOME, and its
 * clang/llvm-ar are handed to cargo through the per-target environment
 * variables, which avoids requiring `cargo-ndk` to be installed.
 */
val buildRustCore by tasks.registering {
    description = "Builds the Rust protocol core for the Android ABIs."
    group = "build"

    val requestedAbis = (providers.gradleProperty("rustAbis").orNull
        ?: "arm64-v8a,x86_64").split(",").map { it.trim() }.filter { it.isNotEmpty() }

    inputs.dir(File(rustDir, "src"))
    inputs.file(File(rustDir, "Cargo.toml"))
    outputs.dir(jniLibsDir)

    doLast {
        val ndkHome = resolveNdkHome()
            ?: throw GradleException(
                "Android NDK not found. Set ANDROID_NDK_HOME, or ndk.dir in " +
                    "local.properties, or install it via the SDK manager.",
            )
        val binDir = File(ndkHome, "toolchains/llvm/prebuilt/${ndkHostTag()}/bin")
        if (!binDir.isDirectory) {
            throw GradleException("NDK prebuilt toolchain missing at $binDir")
        }

        val windows = OperatingSystem.current().isWindows
        val exe = if (windows) ".exe" else ""
        val cmd = if (windows) ".cmd" else ""

        requestedAbis.forEach { abi ->
            val triple = abiToTriple[abi]
                ?: throw GradleException(
                    "No Rust target known for ABI '$abi' (known: ${abiToTriple.keys})",
                )

            val clangName = when (triple) {
                "aarch64-linux-android" -> "aarch64-linux-android$rustApiLevel-clang"
                "x86_64-linux-android" -> "x86_64-linux-android$rustApiLevel-clang"
                else -> throw GradleException("unsupported target triple $triple")
            }
            val linker = File(binDir, "$clangName$cmd")
            if (!linker.isFile) {
                throw GradleException(
                    "NDK linker not found: $linker (is the NDK complete?)",
                )
            }
            // The NDK ships no per-target `*-ar`; ring's build script asks for
            // one, so point it at the generic llvm-ar.
            val archiver = File(binDir, "llvm-ar$exe")
            val ranlib = File(binDir, "llvm-ranlib$exe")

            val envTriple = triple.uppercase().replace('-', '_')
            exec {
                workingDir = rustDir
                commandLine("cargo", "build", "--release", "--target", triple)
                environment("CARGO_TARGET_${envTriple}_LINKER", linker.absolutePath)
                environment("CC_$triple", linker.absolutePath)
                environment("AR_$triple", archiver.absolutePath)
                environment("AR_$envTriple", archiver.absolutePath)
                environment("RANLIB_$envTriple", ranlib.absolutePath)
                environment("ANDROID_NDK_HOME", ndkHome.absolutePath)
            }

            val built = File(rustDir, "target/$triple/release/libmykvm_core.so")
            if (!built.isFile) {
                throw GradleException("cargo did not produce $built")
            }
            val destination = File(jniLibsDir, abi)
            destination.mkdirs()
            built.copyTo(File(destination, "libmykvm_core.so"), overwrite = true)
            logger.lifecycle("Rust core ($abi) -> ${File(destination, "libmykvm_core.so")}")
        }
    }
}

fun resolveNdkHome(): File? {
    val candidates = listOfNotNull(
        providers.gradleProperty("ndk.dir").orNull?.let(::File),
        (System.getenv("ANDROID_NDK_HOME") ?: System.getenv("NDK_HOME"))?.let(::File),
        System.getenv("ANDROID_HOME")?.let { latestNdk(File(it, "ndk")) },
        System.getenv("ANDROID_SDK_ROOT")?.let { latestNdk(File(it, "ndk")) },
    )
    return candidates.firstOrNull { it.isDirectory }
}

fun latestNdk(root: File): File? {
    if (!root.isDirectory) return null
    return root.listFiles()?.filter { it.isDirectory }?.maxByOrNull { it.name }
}

fun ndkHostTag(): String {
    val os = OperatingSystem.current()
    return when {
        os.isWindows -> "windows-x86_64"
        os.isMacOsX -> "darwin-x86_64"
        else -> "linux-x86_64"
    }
}

tasks.named("preBuild") {
    dependsOn(buildRustCore)
}

tasks.matching { it.name == "clean" }.configureEach {
    doLast {
        File(rustDir, "target").deleteRecursively()
    }
}