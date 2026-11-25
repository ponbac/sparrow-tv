import groovy.json.JsonSlurper
import org.gradle.api.artifacts.dsl.LockMode

plugins {
    id("com.android.application")
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
val verifierMaven = file(verifier["manifest_path"] as String).parentFile.resolve("maven")
val verifierVersion = verifier["version"] as String

repositories {
    google()
    mavenCentral()
    exclusiveContent {
        forRepository {
            maven {
                url = uri(verifierMaven)
                metadataSources { artifact() }
            }
        }
        filter { includeModule("rustls", "rustls-platform-verifier") }
    }
}

val nativeLibraries = providers.environmentVariable("SPARROW_SHIELD_JNI_LIBS")
val launcherResources = layout.buildDirectory.dir("generated/sparrow-launcher-res")
val stageLauncherResources = tasks.register<Sync>("stageLauncherResources") {
    from(rootProject.file("../../src-tauri/gen/android/app/src/main/res")) {
        include(
            "drawable-xhdpi/tv_banner.png",
            "mipmap-*/ic_launcher*.png",
            "mipmap-anydpi-v26/ic_launcher*.xml",
            "xml/data_extraction_rules.xml",
        )
    }
    into(launcherResources)
}

android {
    namespace = "xyz.ponbac.sparrow.shield"
    compileSdk = 36
    buildToolsVersion = "35.0.0"
    ndkVersion = "29.0.14206865"

    defaultConfig {
        applicationId = "xyz.ponbac.sparrow.shield"
        minSdk = 26
        targetSdk = 36
        versionCode = 1
        versionName = "0.1.0"
        manifestPlaceholders["fixtureCommandsEnabled"] =
            providers.environmentVariable("SPARROW_SHIELD_FIXTURE").orNull == "1"
        ndk { abiFilters += "arm64-v8a" }
    }

    buildTypes {
        release {
            isDebuggable = false
            isMinifyEnabled = false
            signingConfig = signingConfigs.getByName("debug")
        }
    }
    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    sourceSets.named("main") {
        res.srcDir(launcherResources)
        jniLibs.setSrcDirs(nativeLibraries.orNull?.let(::listOf).orEmpty())
    }
}

tasks.named("preBuild") {
    dependsOn(stageLauncherResources)
}

val verifyNativeLibrary = tasks.register("verifyNativeLibrary") {
    doLast {
        val directory = nativeLibraries.orNull?.let(::file)
        require(directory?.resolve("arm64-v8a/libsparrow_shield.so")?.isFile == true) {
            "Set SPARROW_SHIELD_JNI_LIBS to the directory containing arm64-v8a/libsparrow_shield.so"
        }
    }
}

tasks.matching { it.name == "mergeReleaseJniLibFolders" }.configureEach {
    dependsOn(verifyNativeLibrary)
}

dependencies {
    implementation("rustls:rustls-platform-verifier:$verifierVersion@aar")
    // The verifier's AAR uses Kotlin, and the Java Activity does not apply the Kotlin plugin.
    implementation("org.jetbrains.kotlin:kotlin-stdlib:2.2.10")
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
