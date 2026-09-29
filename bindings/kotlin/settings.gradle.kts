// The Android library: the bridge sources of the React Native module, built
// without React Native. See README.md and ADR 0025.

pluginManagement {
    // The library is built by the toolchain the module's tests run on, which
    // the test harness declares. It is read from there and not written
    // again: a second copy is a compiler somebody raises in one place, and
    // then the code that was tested is not the code that was published.
    //
    // Inside this block because Gradle evaluates it before the rest of the
    // script, and on its own.
    val harness = File(rootDir, "../react-native/android-ci-harness/build.gradle")

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
    // A repository declared by a module is one nobody reviewing this file
    // sees, and this build's output is published.
    repositoriesMode.set(RepositoriesMode.FAIL_ON_PROJECT_REPOS)
    repositories {
        google()
        mavenCentral()
    }
}

rootProject.name = "offline-protocol-kotlin"

include(":offline-protocol-android")
