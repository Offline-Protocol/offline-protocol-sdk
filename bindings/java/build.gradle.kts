// The Java binding. It calls the C API (bindings/c) through JNA, so it needs
// no native code of its own: the native library is the one every desktop
// release already ships. Java 11 or later.

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
    api("net.java.dev.jna:jna:5.14.0")

    testImplementation(platform("org.junit:junit-bom:5.10.2"))
    testImplementation("org.junit.jupiter:junit-jupiter")
    testRuntimeOnly("org.junit.platform:junit-platform-launcher")
}

tasks.withType<JavaCompile>().configureEach {
    options.release.set(11)
    options.compilerArgs.addAll(listOf("-Xlint:all", "-Werror"))
}

tasks.withType<Javadoc>().configureEach {
    (options as StandardJavadocDocletOptions).addBooleanOption("Xdoclint:all,-missing", true)
}

// The tests load the real library. -PnativeLibraryDir names the directory that
// holds liboffline_protocol_uniffi.{so,dylib} (offline_protocol_uniffi.dll);
// the default is the Cargo debug output of this checkout.
tasks.test {
    useJUnitPlatform()
    val libDir = providers.gradleProperty("nativeLibraryDir")
        .orElse(layout.projectDirectory.dir("../../target/debug").asFile.absolutePath)
    systemProperty("jna.library.path", libDir.get())
    testLogging {
        events("failed", "skipped")
        exceptionFormat = org.gradle.api.tasks.testing.logging.TestExceptionFormat.FULL
    }
}
