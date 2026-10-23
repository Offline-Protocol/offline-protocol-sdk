// The example depends on the Java binding by its published coordinate. The
// composite build below substitutes the checkout's copy for it, so the example
// runs against the source in this repository. In your own project, drop this
// block and depend on the version you want from Maven Central.
pluginManagement {
    repositories {
        gradlePluginPortal()
    }
}

dependencyResolutionManagement {
    repositories {
        mavenCentral()
    }
}

rootProject.name = "offline-protocol-java-example"

includeBuild("../../bindings/java") {
    dependencySubstitution {
        substitute(module("com.offlineprotocol:offline-protocol-java")).using(project(":"))
    }
}
