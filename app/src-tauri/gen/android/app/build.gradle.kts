import groovy.json.JsonSlurper
import org.gradle.api.artifacts.dsl.LockMode
import java.io.File
import java.util.Properties

plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
    id("rust")
}

val tauriProperties = Properties().apply {
    val propFile = file("tauri.properties")
    if (propFile.exists()) {
        propFile.inputStream().use { load(it) }
    }
}

data class RustlsPlatformVerifierProject(val repository: File, val version: String)

fun findRustlsPlatformVerifierProject(): RustlsPlatformVerifierProject {
    val manifest = file("../../../Cargo.toml").canonicalFile
    val metadata = providers.exec {
        workingDir = manifest.parentFile
        commandLine(
            "cargo",
            "metadata",
            "--locked",
            "--format-version",
            "1",
            "--filter-platform",
            "aarch64-linux-android",
            "--manifest-path",
            manifest.absolutePath,
        )
    }.standardOutput.asText.get()
    val packages = (JsonSlurper().parseText(metadata) as Map<*, *>)["packages"] as List<*>
    val verifier = packages
        .filterIsInstance<Map<*, *>>()
        .first { it["name"] == "rustls-platform-verifier-android" }
    val verifierManifest = file(verifier["manifest_path"] as String)
    return RustlsPlatformVerifierProject(
        File(verifierManifest.parentFile, "maven"),
        verifier["version"] as String,
    )
}

val rustlsPlatformVerifierProject = findRustlsPlatformVerifierProject()

val releaseSigningValues = mapOf(
    "storeFile" to System.getenv("SPARROW_ANDROID_KEYSTORE_PATH"),
    "storePassword" to System.getenv("SPARROW_ANDROID_KEYSTORE_PASSWORD"),
    "keyAlias" to System.getenv("SPARROW_ANDROID_KEY_ALIAS"),
    "keyPassword" to System.getenv("SPARROW_ANDROID_KEY_PASSWORD"),
)
val hasCompleteReleaseSigning = releaseSigningValues.values.all { !it.isNullOrBlank() }
val hasPartialReleaseSigning = releaseSigningValues.values.any { !it.isNullOrBlank() }
check(!hasPartialReleaseSigning || hasCompleteReleaseSigning) {
    "Android release signing configuration is incomplete"
}

repositories {
    maven {
        url = uri(rustlsPlatformVerifierProject.repository)
        metadataSources { artifact() }
    }
}

android {
    compileSdk = 36
    buildToolsVersion = "35.0.0"
    ndkVersion = "29.0.14206865"
    namespace = "xyz.ponbac.sparrow"
    defaultConfig {
        manifestPlaceholders["usesCleartextTraffic"] = "false"
        applicationId = "xyz.ponbac.sparrow"
        minSdk = 24
        targetSdk = 36
        versionCode = tauriProperties.getProperty("tauri.android.versionCode", "1").toInt()
        versionName = tauriProperties.getProperty("tauri.android.versionName", "1.0")
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
        getByName("debug") {
            manifestPlaceholders["usesCleartextTraffic"] = "true"
            isDebuggable = true
            isJniDebuggable = true
            isMinifyEnabled = false
            packaging {
                jniLibs.keepDebugSymbols.add("*/arm64-v8a/*.so")
                jniLibs.keepDebugSymbols.add("*/armeabi-v7a/*.so")
                jniLibs.keepDebugSymbols.add("*/x86/*.so")
                jniLibs.keepDebugSymbols.add("*/x86_64/*.so")
            }
        }
        getByName("release") {
            if (hasCompleteReleaseSigning) {
                signingConfig = signingConfigs.getByName("release")
            }
            isMinifyEnabled = true
            proguardFiles(
                *fileTree(".") { include("**/*.pro") }
                    .plus(getDefaultProguardFile("proguard-android-optimize.txt"))
                    .toList().toTypedArray()
            )
        }
    }
    kotlinOptions {
        jvmTarget = "1.8"
    }
    buildFeatures {
        buildConfig = true
    }
}

gradle.taskGraph.whenReady {
    val releaseOutputTask = Regex("^(?:assemble|package|bundle).+Release$", RegexOption.IGNORE_CASE)
    val requestsReleaseOutput = gradle.startParameter.taskNames.any { requestedTask ->
        releaseOutputTask.matches(requestedTask.substringAfterLast(':'))
    }
    check(!requestsReleaseOutput || hasCompleteReleaseSigning) {
        "Android release signing configuration is required"
    }
}

rust {
    rootDirRel = "../../../"
}

dependencies {
    val media3Version = "1.11.0"

    implementation(
        "rustls:rustls-platform-verifier:${rustlsPlatformVerifierProject.version}@aar"
    )
    implementation("androidx.media3:media3-exoplayer:$media3Version")
    implementation("androidx.media3:media3-ui:$media3Version")
    implementation("androidx.webkit:webkit:1.14.0")
    implementation("androidx.appcompat:appcompat:1.7.1")
    implementation("androidx.activity:activity-ktx:1.10.1")
    implementation("com.google.android.material:material:1.12.0")
    implementation("androidx.lifecycle:lifecycle-process:2.10.0")
    testImplementation("junit:junit:4.13.2")
    androidTestImplementation("androidx.test.ext:junit:1.1.4")
    androidTestImplementation("androidx.test.espresso:espresso-core:3.5.0")
}

val lockedBuildClasspaths = listOf("arm64", "arm", "x86", "x86_64", "universal")
    .flatMap { abi ->
        listOf("Debug", "Release").flatMap { buildType ->
            listOf("Compile", "Runtime").map { usage ->
                "$abi$buildType${usage}Classpath"
            }
        }
    }

dependencyLocking {
    lockMode.set(LockMode.STRICT)
}

configurations.configureEach {
    if (name in lockedBuildClasspaths) {
        resolutionStrategy.activateDependencyLocking()
    }
}

tasks.register("resolveAndLockBuildClasspaths") {
    notCompatibleWithConfigurationCache("Resolves the supported app build classpaths")
    doFirst {
        require(gradle.startParameter.isWriteDependencyLocks) {
            "$path must be run with --write-locks"
        }
        val missing = lockedBuildClasspaths.filterNot(configurations.names::contains)
        require(missing.isEmpty()) {
            "Missing lockable app build classpaths: ${missing.joinToString()}"
        }
    }
    doLast {
        lockedBuildClasspaths.sorted().forEach { name ->
            val components = configurations.getByName(name)
                .incoming.resolutionResult.allComponents
            require(components.isNotEmpty()) { "$name resolved no dependency components" }
        }
    }
}

apply(from = "tauri.build.gradle.kts")
