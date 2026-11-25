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

val guideNotices = layout.buildDirectory.dir("generated/guide-notices")
val stageGuideNotices = tasks.register<Sync>("stageGuideNotices") {
    val guideAssets = rootProject.file("../../../crates/sparrow-shield/assets")
    from(guideAssets.resolve("fonts/OFL-Saira.txt"))
    from(guideAssets.resolve("icons/LICENSE-Lucide.txt"))
    into(guideNotices.map { it.dir("licenses") })
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
        java.srcDir(rootProject.file("../../android-playback/src/main/java"))
        res.srcDir(launcherResources)
        assets.srcDir(guideNotices)
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
    dependsOn(stageLauncherResources, stageGuideNotices)
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
    val media3Version = "1.11.0"

    implementation("org.rustls:rustls-platform-verifier:$verifierVersion")
    implementation("org.jetbrains.kotlin:kotlin-stdlib:2.2.10")
    implementation("androidx.annotation:annotation:1.10.0")
    implementation("androidx.annotation:annotation-experimental:1.4.1")
    implementation("androidx.media3:media3-exoplayer:$media3Version")
    implementation("androidx.media3:media3-ui:$media3Version")
    // The bundled audio decoder tracks the Media3 version in both Android hosts.
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
