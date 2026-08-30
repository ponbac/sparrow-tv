import org.gradle.api.artifacts.dsl.LockMode

plugins {
    `kotlin-dsl`
}

gradlePlugin {
    plugins {
        create("pluginsForCoolKids") {
            id = "rust"
            implementationClass = "RustPlugin"
        }
    }
}

repositories {
    google()
    mavenCentral()
}

dependencies {
    compileOnly(gradleApi())
    implementation("com.android.tools.build:gradle:8.11.0")
}

val lockedBuildClasspaths = listOf(
    "compileClasspath",
    "runtimeClasspath",
    "testCompileClasspath",
    "testRuntimeClasspath",
)

dependencyLocking {
    lockMode.set(LockMode.STRICT)
}

configurations.configureEach {
    if (name in lockedBuildClasspaths) {
        resolutionStrategy.activateDependencyLocking()
    }
}

tasks.register("resolveAndLockBuildClasspaths") {
    notCompatibleWithConfigurationCache("Resolves the buildSrc build classpaths")
    doFirst {
        require(gradle.startParameter.isWriteDependencyLocks) {
            "$path must be run with --write-locks"
        }
        val missing = lockedBuildClasspaths.filterNot(configurations.names::contains)
        require(missing.isEmpty()) {
            "Missing lockable buildSrc classpaths: ${missing.joinToString()}"
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
