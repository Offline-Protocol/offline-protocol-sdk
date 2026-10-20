// The Java binding. The sources under src/main/java are generated from the UDL by
// scripts/generate-bindings.sh and committed, like the Swift, Kotlin and Python
// bindings; they reach the native library through the foreign function API
// (final in Java 22), so there is no JNA and no native glue.
//
// Build with JDK 22 or later. The tests load the native library, so build it first.

plugins {
    `java-library`
    `maven-publish`
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

// The notices the Rust library's licenses ask to travel with it, at the root of
// the jar's META-INF as the Android library carries them at the root of its AAR.
// Read from the repository root, not copied: a copy is a notice someone updates
// in one place. The jar holds no native code, so these cover the Java and the
// library the host supplies alongside it.
val legalFiles = listOf("LICENSE", "LICENSE-COMMERCIAL.md", "THIRD-PARTY-NOTICES.md", "EXPORT.md")
val repositoryRoot = layout.projectDirectory.dir("../..")

legalFiles.forEach { name ->
    require(repositoryRoot.file(name).asFile.isFile) {
        "legalFiles names $name, which is not at ${repositoryRoot.asFile}"
    }
}

tasks.jar {
    from(repositoryRoot) {
        include(legalFiles)
        into("META-INF")
    }
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

// com.offlineprotocol:offline-protocol-java. The Android library is
// offline-protocol-sdk; this one is a different artifact for a different
// runtime, so the two never share a coordinate.
publishing {
    publications {
        create<MavenPublication>("java") {
            from(components["java"])

            groupId = "com.offlineprotocol"
            artifactId = "offline-protocol-java"

            pom {
                name.set("Offline Protocol SDK for Java")
                description.set(
                    "Offline-first messaging for the JVM: BLE mesh and internet relay " +
                        "transports, with MLS end-to-end encryption (RFC 9420) applied " +
                        "automatically. Needs JDK 22 or later, and the native library for " +
                        "the platform, which this jar does not contain."
                )
                url.set("https://github.com/Offline-Protocol/offline-protocol-sdk")
                licenses {
                    license {
                        name.set("AGPL-3.0-only")
                        url.set("https://www.gnu.org/licenses/agpl-3.0.txt")
                        distribution.set("repo")
                        comments.set(
                            "A commercial license is available. See " +
                                "LICENSE-COMMERCIAL.md in the source repository."
                        )
                    }
                }
                developers {
                    developer {
                        id.set("offline-protocol")
                        name.set("Offline Protocol, Inc.")
                        url.set("https://www.offlineprotocol.com")
                    }
                }
                scm {
                    url.set("https://github.com/Offline-Protocol/offline-protocol-sdk")
                    connection.set(
                        "scm:git:https://github.com/Offline-Protocol/offline-protocol-sdk.git"
                    )
                    developerConnection.set(
                        "scm:git:ssh://git@github.com/Offline-Protocol/offline-protocol-sdk.git"
                    )
                }
            }
        }
    }

    // Where the library is written as a Maven repository, whatever it is later
    // uploaded to. The release signs and uploads this; the key never enters a
    // Gradle build (scripts/maven-central-bundle.sh).
    repositories {
        maven {
            name = "staging"
            url = uri(layout.buildDirectory.dir("maven"))
        }
    }
}
