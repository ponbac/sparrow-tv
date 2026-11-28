import groovy.json.JsonSlurper
import org.gradle.api.artifacts.dsl.LockMode
import org.jetbrains.kotlin.gradle.dsl.JvmTarget

plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
}

val cargoManifest = rootProject.file("../../../Cargo.toml")
val cargoMetadata = providers.exec {
    workingDir = cargoManifest.parentFile
    commandLine(
        "cargo", "metadata", "--locked", "--format-version", "1",
        "--filter-platform", "aarch64-linux-android", "--manifest-path", cargoManifest.absolutePath,
    )
}.standardOutput.asText.get()
val cargoPackages = (JsonSlurper().parseText(cargoMetadata) as Map<*, *>)["packages"] as List<*>
val verifier = cargoPackages.filterIsInstance<Map<*, *>>()
    .single { it["name"] == "rustls-platform-verifier-android" }
val verifierVersion = verifier["version"] as String

repositories {
    google()
    mavenCentral()
    exclusiveContent {
        forRepository {
            maven {
                // Pin the archive revision alongside the Cargo-selected artifact version.
                url = uri("https://raw.githubusercontent.com/rustls/rustls-platform-verifier/1aa691352a5e0c215210dfc9a3c2f2078639dc91/android-release-support/maven/")
            }
        }
        filter { includeModule("org.rustls", "rustls-platform-verifier") }
    }
}

val product = JsonSlurper().parse(rootProject.file("../../package.json")) as Map<*, *>
val productVersion = product["version"] as String
require(Regex("(0|[1-9][0-9]{0,2})[.](0|[1-9][0-9]{0,2})[.](0|[1-9][0-9]{0,2})").matches(productVersion)) {
    "app/package.json must contain a stable Sparrow version"
}
val (major, minor, patch) = productVersion.split('.').map(String::toInt)
require(major + minor + patch > 0) { "Sparrow version must be greater than 0.0.0" }

val toolchain = JsonSlurper().parse(rootProject.file("../../../release/toolchain.json")) as Map<*, *>
val androidToolchain = toolchain["android"] as Map<*, *>
val releaseAbis = (androidToolchain["abis"] as List<*>).map { it as String }
val debugAbi = providers.environmentVariable("SPARROW_SHIELD_ABI").getOrElse("arm64-v8a")
require(debugAbi in releaseAbis) { "Unsupported Shield development ABI: $debugAbi" }

val nativeLibraries = providers.environmentVariable("SPARROW_SHIELD_JNI_LIBS")
val releaseSigningValues = mapOf(
    "storeFile" to providers.environmentVariable("SPARROW_ANDROID_KEYSTORE_PATH").orNull,
    "storePassword" to providers.environmentVariable("SPARROW_ANDROID_KEYSTORE_PASSWORD").orNull,
    "keyAlias" to providers.environmentVariable("SPARROW_ANDROID_KEY_ALIAS").orNull,
    "keyPassword" to providers.environmentVariable("SPARROW_ANDROID_KEY_PASSWORD").orNull,
)
val hasCompleteReleaseSigning = releaseSigningValues.values.all { !it.isNullOrBlank() }

val guideAssetsDir = layout.buildDirectory.dir("generated/guide-assets")
val stageGuideAssets = tasks.register<Sync>("stageGuideAssets") {
    val guideAssets = rootProject.file("../../../crates/sparrow-shield/assets")
    from(guideAssets.resolve("fonts/sparrow-guide-80-700.ttf"))
    from(guideAssets.resolve("fonts/OFL-Saira.txt")) { into("licenses") }
    from(guideAssets.resolve("icons/LICENSE-Lucide.txt")) { into("licenses") }
    into(guideAssetsDir)
}

android {
    namespace = "xyz.ponbac.sparrow.shield"
    compileSdk = (androidToolchain["platform"] as String).removePrefix("android-").toInt()
    buildToolsVersion = androidToolchain["buildTools"] as String
    ndkVersion = androidToolchain["ndk"] as String

    defaultConfig {
        applicationId = "xyz.ponbac.sparrow"
        minSdk = androidToolchain["minSdk"] as Int
        targetSdk = androidToolchain["targetSdk"] as Int
        versionCode = major * 1_000_000 + minor * 1_000 + patch
        versionName = productVersion
        manifestPlaceholders["appLabel"] = "Sparrow"
        manifestPlaceholders["fixtureCommandsEnabled"] = false
    }

    signingConfigs {
        if (hasCompleteReleaseSigning) {
            create("release") {
                storeFile = file(checkNotNull(releaseSigningValues["storeFile"]))
                storePassword = checkNotNull(releaseSigningValues["storePassword"])
                keyAlias = checkNotNull(releaseSigningValues["keyAlias"])
                keyPassword = checkNotNull(releaseSigningValues["keyPassword"])
            }
        }
    }

    buildTypes {
        debug {
            applicationIdSuffix = ".shield"
            manifestPlaceholders["appLabel"] = "Sparrow Shield (Dev)"
            manifestPlaceholders["fixtureCommandsEnabled"] =
                providers.environmentVariable("SPARROW_SHIELD_FIXTURE").orNull == "1"
            ndk { abiFilters += debugAbi }
        }
        release {
            isMinifyEnabled = false // Rust JNI calls Java methods by name.
            signingConfig = signingConfigs.findByName("release")
            ndk { abiFilters += releaseAbis }
        }
    }
    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    sourceSets.named("main") {
        java.srcDir(rootProject.file("../../android-playback/src/main/java"))
        assets.srcDir(guideAssetsDir)
        jniLibs.setSrcDirs(nativeLibraries.orNull?.let(::listOf).orEmpty())
    }
    sourceSets.named("test") {
        java.srcDir(rootProject.file("../../android-playback/src/test/java"))
    }
}

kotlin {
    compilerOptions {
        jvmTarget.set(JvmTarget.JVM_17)
    }
}

tasks.named("preBuild") {
    dependsOn(stageGuideAssets)
}

gradle.taskGraph.whenReady {
    val releaseOutputTasks = setOf(
        ":app:assembleRelease", ":app:packageRelease", ":app:bundleRelease",
        ":app:signReleaseBundle", ":app:validateSigningRelease",
    )
    if (allTasks.any { it.path in releaseOutputTasks }) {
        check(hasCompleteReleaseSigning) { "Android release signing configuration is required" }
        check(file(checkNotNull(releaseSigningValues["storeFile"])).isFile) {
            "Android release keystore must be a file"
        }
        check(providers.environmentVariable("SPARROW_SHIELD_FIXTURE").orNull != "1") {
            "Android release builds exclude fixture commands"
        }
    }
}

mapOf("Debug" to listOf(debugAbi), "Release" to releaseAbis).forEach { (variant, abis) ->
    val verifyLibrary = tasks.register("verify${variant}NativeLibrary") {
        doLast {
            val directory = nativeLibraries.orNull?.let(::file)
            for (abi in abis) {
                require(directory?.resolve("$abi/libsparrow_shield.so")?.isFile == true) {
                    "Set SPARROW_SHIELD_JNI_LIBS to the directory containing $abi/libsparrow_shield.so"
                }
            }
        }
    }
    tasks.matching { it.name == "merge${variant}JniLibFolders" }.configureEach {
        dependsOn(verifyLibrary)
    }
}

dependencies {
    val media3Version = "1.11.0"

    implementation("org.rustls:rustls-platform-verifier:$verifierVersion")
    implementation("org.jetbrains.kotlin:kotlin-stdlib:2.2.10")
    implementation("androidx.annotation:annotation:1.10.0")
    implementation("androidx.annotation:annotation-experimental:1.4.1")
    implementation("androidx.core:core:1.13.1")
    implementation("androidx.media3:media3-exoplayer:$media3Version")
    implementation("androidx.media3:media3-ui:$media3Version")
    // The bundled audio decoder tracks the Media3 version in the Android host.
    implementation("io.github.anilbeesetti:nextlib-media3ext:$media3Version-0.15.0")
    constraints {
        // Keep the playback UI's AndroidX runtime versions aligned with the installed app.
        implementation("androidx.collection:collection:1.1.0")
        implementation("androidx.core:core-ktx:1.13.1")
        implementation("androidx.customview:customview:1.1.0")
        implementation("androidx.lifecycle:lifecycle-runtime:2.10.0")
    }
    testImplementation("junit:junit:4.13.2")
}

dependencyLocking {
    lockMode.set(LockMode.STRICT)
}

val buildClasspaths = listOf("debug", "release").flatMap { buildType ->
    listOf("Compile", "Runtime").map { usage -> "$buildType${usage}Classpath" }
}
configurations.configureEach {
    if (name in buildClasspaths) {
        resolutionStrategy.activateDependencyLocking()
    }
}

tasks.register("resolveAndLockBuildClasspaths") {
    notCompatibleWithConfigurationCache("Resolves the supported app build classpaths")
    doFirst {
        require(gradle.startParameter.isWriteDependencyLocks) {
            "$path must be run with --write-locks"
        }
    }
    doLast {
        buildClasspaths.forEach { name ->
            configurations.getByName(name).resolve()
        }
    }
}
