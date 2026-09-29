// An application's view of the library: one dependency line, resolved from
// the Maven repository the library's build wrote, in a release build that
// minifies.
//
// A separate build on purpose. A module of the library's own build would
// depend on the project, and what needs checking is the published artifact:
// that its metadata resolves, that what it depends on arrives with it, that
// its public API compiles for someone who declared nothing else, and that
// the rules it ships are enough for R8.

pluginManagement {
    // The toolchain the library is built with, read from where the library
    // reads it (../settings.gradle.kts).
    val harness = File(rootDir, "../../react-native/android-ci-harness/build.gradle")

    fun fromTheHarness(artifact: String): String {
        val pattern = Regex(
            """(?m)^[ \t]*classpath[ \t(]+["']""" + Regex.escape(artifact) +
                """:([^"':@${'$'}\s]+)["']"""
        )
        val found = pattern.findAll(harness.readText()).map { it.groupValues[1] }.toList()
        if (found.size != 1) {
            throw GradleException(
                "expected one `classpath \"$artifact:<version>\"` in $harness " +
                    "and found ${found.size}"
            )
        }
        return found.single()
    }

    val androidGradlePlugin = fromTheHarness("com.android.tools.build:gradle")
    val kotlinGradlePlugin = fromTheHarness("org.jetbrains.kotlin:kotlin-gradle-plugin")

    repositories {
        google()
        mavenCentral()
        gradlePluginPortal()
    }

    resolutionStrategy {
        eachPlugin {
            when (requested.id.id) {
                "com.android.library", "com.android.application" ->
                    useModule("com.android.tools.build:gradle:$androidGradlePlugin")
                "org.jetbrains.kotlin.android" ->
                    useModule("org.jetbrains.kotlin:kotlin-gradle-plugin:$kotlinGradlePlugin")
            }
        }
    }
}

dependencyResolutionManagement {
    repositoriesMode.set(RepositoriesMode.FAIL_ON_PROJECT_REPOS)
    repositories {
        // The library comes from the directory the build beside this one
        // wrote, and from nowhere else. Without `exclusiveContent` a
        // repository further down is asked too when this one has no such
        // version, and once the library is published somewhere that answer
        // would be the released artifact, passing for the one under test.
        exclusiveContent {
            forRepository {
                maven {
                    name = "staging"
                    url = uri("../offline-protocol-android/build/maven")
                }
            }
            filter { includeGroup("com.offlineprotocol") }
        }
        google()
        mavenCentral()
    }
}

rootProject.name = "offline-protocol-consumer-check"

include(":app")
