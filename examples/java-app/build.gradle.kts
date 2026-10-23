plugins {
    application
}

// Run Gradle on JDK 22 or later: the foreign function API the binding uses is
// final in Java 22.
tasks.withType<JavaCompile>().configureEach {
    options.release.set(22)
}

dependencies {
    implementation("com.offlineprotocol:offline-protocol-java:0.0.0")
    // The engine reports events as JSON; any JSON library reads them.
    implementation("com.fasterxml.jackson.core:jackson-databind:2.17.2")
}

application {
    mainClass.set("com.example.offlineprotocol.Main")
    // The binding calls native code, and the JVM asks permission for that.
    applicationDefaultJvmArgs = listOf("--enable-native-access=ALL-UNNAMED")
}

// The native library the binding loads. Build it first (cargo build -p
// offline-protocol-uniffi); -PnativeLibrary names another file.
tasks.named<JavaExec>("run") {
    val os = System.getProperty("os.name").lowercase()
    val fileName = when {
        os.contains("win") -> "offline_protocol_uniffi.dll"
        os.contains("mac") -> "liboffline_protocol_uniffi.dylib"
        else -> "liboffline_protocol_uniffi.so"
    }
    val library = providers.gradleProperty("nativeLibrary")
        .orElse(layout.projectDirectory.file("../../target/debug/$fileName").asFile.absolutePath)
    systemProperty("uniffi.component.offline_protocol.libraryOverride", library.get())
}
