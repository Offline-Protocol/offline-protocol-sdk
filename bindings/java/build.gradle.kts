// The Java binding. The sources under src/main/java are generated from the UDL by
// scripts/generate-bindings.sh and committed, like the Swift, Kotlin and Python
// bindings; they reach the native library through the foreign function API
// (final in Java 22), so there is no JNA and no native glue.
//
// Build with JDK 22 or later. The tests load the native library, so build it first.

plugins {
    `java-library`
}

group = "com.offlineprotocol"
version = providers.gradleProperty("VERSION_NAME").get()

java {
    withSourcesJar()
    withJavadocJar()
}

dependencies {
    testImplementation(platform("org.junit:junit-bom:5.10.2"))
    testImplementation("org.junit.jupiter:junit-jupiter")
    testRuntimeOnly("org.junit.platform:junit-platform-launcher")
}

tasks.withType<JavaCompile>().configureEach {
    options.release.set(22)
}

// The generated Javadoc is not ours to fix up: it carries no comments, and the
// generator's own warnings would fail a -Werror build for nothing we wrote.
tasks.withType<Javadoc>().configureEach {
    (options as StandardJavadocDocletOptions).addBooleanOption("Xdoclint:none", true)
}

// The tests load the real library. -PnativeLibrary names the file itself;
// the default is this checkout's Cargo debug output for the host.
tasks.test {
    useJUnitPlatform()
    val os = System.getProperty("os.name").lowercase()
    val fileName = when {
        os.contains("win") -> "offline_protocol_uniffi.dll"
        os.contains("mac") -> "liboffline_protocol_uniffi.dylib"
        else -> "liboffline_protocol_uniffi.so"
    }
    val library = providers.gradleProperty("nativeLibrary")
        .orElse(layout.projectDirectory.file("../../target/debug/$fileName").asFile.absolutePath)
    // The generated loader reads this property, then System.load()s an absolute path.
    systemProperty("uniffi.component.offline_protocol.libraryOverride", library.get())
    jvmArgs("--enable-native-access=ALL-UNNAMED")
    testLogging {
        events("failed", "skipped")
        exceptionFormat = org.gradle.api.tasks.testing.logging.TestExceptionFormat.FULL
    }
}
