import com.android.build.api.artifact.SingleArtifact
import javax.xml.parsers.DocumentBuilderFactory
import javax.xml.transform.OutputKeys
import javax.xml.transform.TransformerFactory
import javax.xml.transform.dom.DOMSource
import javax.xml.transform.stream.StreamResult
import org.jetbrains.kotlin.gradle.tasks.KotlinCompile
import org.w3c.dom.Element
import org.w3c.dom.Node

plugins {
    id("com.android.library")
    id("org.jetbrains.kotlin.android")
    id("maven-publish")
    id("signing")
}

// The bridge sources have one home, the React Native module, and this build
// takes them from there (ADR 0025).
val bridge = layout.projectDirectory.dir("../../react-native/android")
val bridgePackage = "src/main/java/com/offlineprotocol"

// What needs React. A list of what to leave out, not of what to take: a new
// bridge source is in the library without being added here.
val reactOnly = listOf(
    "OfflineProtocolModule.kt",
    "OfflineProtocolPackage.kt",
    "MeshHeadlessWakeService.kt",
)

// Every name has to match a file. One that matches nothing is a rename nobody
// carried over here, and the renamed file, which still needs React, would be
// compiled into the library.
reactOnly.forEach { name ->
    require(bridge.file("$bridgePackage/$name").asFile.isFile) {
        "reactOnly names $name, which is not in ${bridge.dir(bridgePackage).asFile}"
    }
}

// What the React Native manifest declares for the files above.
val reactOnlyServices = listOf("com.offlineprotocol.MeshHeadlessWakeService")
// Held for React Native's headless task, which takes a wake lock. Nothing in
// the library takes one.
val reactOnlyPermissions = listOf("android.permission.WAKE_LOCK")

// The version is the release tag's and is passed in. There is no number here
// for a release to forget.
val libraryVersion = providers.gradleProperty("VERSION_NAME").orElse("0.0.0-local")

// The module's build file declares the SDK levels and the dependency
// versions for these sources. This build reads them and declares none of its
// own: a second copy is a version somebody raises in one place, and then the
// same transport runs over two different WebSocket libraries.
//
// Read strictly. Each pattern is anchored to a line that declares something,
// so a version in a comment or a commented-out line is not one, and each has
// to match exactly once: two declarations are a question this file cannot
// answer, and the first of them is not an answer.
val moduleBuildFile = bridge.file("build.gradle").asFile
val moduleBuild = moduleBuildFile.readText()

fun exactlyOne(what: String, pattern: String): String {
    val found = Regex(pattern).findAll(moduleBuild).map { it.groupValues[1] }.toList()
    if (found.size != 1) {
        throw GradleException(
            "expected $moduleBuildFile to declare $what exactly once, on a line of its " +
                "own, and found ${found.size} such lines"
        )
    }
    return found.single()
}

fun sdkLevel(name: String): Int =
    exactlyOne(name, """(?m)^[ \t]*${Regex.escape(name)}[ \t]+(\d+)[ \t]*$""").toInt()

/** `group:name`, at the version the module declares in one of `configurations`. */
fun atTheModulesVersion(module: String, vararg configurations: String): String {
    val declaredBy = configurations.joinToString("|") { Regex.escape(it) }
    return "$module:" + exactlyOne(
        "$module in ${configurations.joinToString(" or ")}",
        """(?m)^[ \t]*(?:$declaredBy)[ \t(]+['"]""" + Regex.escape(module) +
            """:([^'":@${'$'}\s]+)(?:@aar)?['"]""",
    )
}

android {
    namespace = "com.offlineprotocol"
    compileSdk = sdkLevel("compileSdk")

    defaultConfig {
        minSdk = sdkLevel("minSdkVersion")
        consumerProguardFiles(bridge.file("consumer-rules.pro"))
    }

    sourceSets {
        getByName("main") {
            manifest.srcFile(bridge.file("src/main/AndroidManifest.xml"))
            java.setSrcDirs(listOf(bridge.dir("src/main/java"), "src/main/java"))
            jniLibs.setSrcDirs(listOf(bridge.dir("src/main/jniLibs")))
        }
        getByName("test") {
            java.setSrcDirs(listOf(bridge.dir("src/test/java"), "src/test/java"))
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    kotlinOptions {
        jvmTarget = "17"
    }

    publishing {
        singleVariant("release") {
            withSourcesJar()
        }
    }

    // A call to an API newer than minSdk compiles, passes every unit test on
    // the JVM, and throws NoClassDefFoundError on an older phone, which a
    // `catch (e: Exception)` does not catch. java.time on Android 7 was one.
    // Lint's NewApi check is the class of that defect, so it runs alone and
    // fails the build: the other checks report findings this library has not
    // decided on. lint.xml leaves out the generated bindings, which guard
    // their one such call behind a Class.forName.
    lint {
        checkOnly += "NewApi"
        abortOnError = true
        lintConfig = file("lint.xml")
    }
}

// A source set is a list of directories and cannot leave a file out, so the
// tasks that read one are told: the compiler, and whatever packs sources.
// Left alone, the sources jar published the React module as the source of a
// library that does not contain it. scripts/check_android_aar.py looks inside
// the jar that is published, because which task packs it is the plugin's to
// change.
tasks.withType<KotlinCompile>().configureEach {
    exclude(reactOnly.map { "**/$it" })
}
tasks.withType<org.gradle.jvm.tasks.Jar>().configureEach {
    exclude(reactOnly.map { "**/$it" })
}

// Rewrites the manifest the library ships: the React Native module's, less
// what it declares for React.
//
// The manifest is not copied into this build and edited, because a copy is a
// second list of permissions for someone to update one of. It fails when an
// entry it was told to remove is not there: the entry was renamed, and under
// its new name it would ship.
abstract class LeaveOutOfManifest : DefaultTask() {
    @get:InputFile
    abstract val merged: RegularFileProperty

    @get:OutputFile
    abstract val updated: RegularFileProperty

    // Not `services`: every task already has a getter of that name.
    @get:Input
    abstract val serviceNames: ListProperty<String>

    @get:Input
    abstract val permissionNames: ListProperty<String>

    @TaskAction
    fun rewrite() {
        val factory = DocumentBuilderFactory.newInstance()
        factory.isNamespaceAware = true
        val document = factory.newDocumentBuilder().parse(merged.get().asFile)
        val android = "http://schemas.android.com/apk/res/android"

        fun remove(tag: String, name: String) {
            val nodes = document.getElementsByTagName(tag)
            val matches = (0 until nodes.length)
                .map { nodes.item(it) as Element }
                .filter { it.getAttributeNS(android, "name") == name }
            if (matches.size != 1) {
                throw GradleException(
                    "expected one <$tag android:name=\"$name\"> in the React Native " +
                        "manifest and found ${matches.size}. If it was renamed, rename it in " +
                        "the lists at the top of the library's build file too, or it ships " +
                        "in the library under its new name"
                )
            }
            matches.single().let { it.parentNode.removeChild(it) }
        }

        serviceNames.get().forEach { remove("service", it) }
        permissionNames.get().forEach { remove("uses-permission", it) }

        // The comments explain the module's manifest to whoever edits it,
        // and some explain the entries just removed. None explains anything
        // to an application that merges this one.
        fun removeComments(node: Node) {
            var child = node.firstChild
            while (child != null) {
                val next = child.nextSibling
                if (child.nodeType == Node.COMMENT_NODE) {
                    node.removeChild(child)
                } else {
                    removeComments(child)
                }
                child = next
            }
        }
        removeComments(document)

        val transformer = TransformerFactory.newInstance().newTransformer()
        transformer.setOutputProperty(OutputKeys.INDENT, "no")
        transformer.transform(DOMSource(document), StreamResult(updated.get().asFile))
    }
}

androidComponents {
    onVariants { variant ->
        val name = variant.name.replaceFirstChar { it.uppercase() }
        val task = tasks.register<LeaveOutOfManifest>("leaveReactOutOf${name}Manifest") {
            serviceNames.set(reactOnlyServices)
            permissionNames.set(reactOnlyPermissions)
        }
        variant.artifacts.use(task)
            .wiredWithFiles(LeaveOutOfManifest::merged, LeaveOutOfManifest::updated)
            .toTransform(SingleArtifact.MERGED_MANIFEST)
    }
}

dependencies {
    // The generated bindings call the Rust library through JNA, and their
    // public types extend JNA's. `api`, so that an application compiles
    // against what it was given without declaring JNA for itself.
    api(atTheModulesVersion("net.java.dev.jna:jna", "implementation") + "@aar")

    // WebSocket, for the internet transport.
    implementation(atTheModulesVersion("com.squareup.okhttp3:okhttp", "implementation"))

    // Keystore-backed storage for MLS material.
    implementation(atTheModulesVersion("androidx.security:security-crypto", "implementation"))

    // The foreground service's notification, and permission checks in the
    // BLE and Wi-Fi Direct managers. The module gets this through React
    // Native and declares it for its tests only, which is the version read
    // here.
    implementation(atTheModulesVersion("androidx.core:core", "testImplementation"))

    testImplementation(atTheModulesVersion("junit:junit", "testImplementation"))
    testImplementation(atTheModulesVersion("org.robolectric:robolectric", "testImplementation"))
    // The org.json in android.jar is a stub that throws.
    testImplementation(atTheModulesVersion("org.json:json", "testImplementation"))
}

// The notices the Rust library's licenses ask to travel with it, packed at
// the root of the AAR, as the Swift package carries them at its root. Read
// from the repository root, not copied: a copy is a notice someone updates in
// one place. scripts/check_android_aar.py refuses an AAR without them.
val legalFiles = listOf("LICENSE", "LICENSE-COMMERCIAL.md", "THIRD-PARTY-NOTICES.md", "EXPORT.md")
val repositoryRoot = layout.projectDirectory.dir("../../..")

legalFiles.forEach { name ->
    require(repositoryRoot.file(name).asFile.isFile) {
        "legalFiles names $name, which is not at ${repositoryRoot.asFile}"
    }
}

tasks.withType<com.android.build.gradle.tasks.BundleAar>().configureEach {
    from(repositoryRoot) {
        include(legalFiles)
    }
}

// A repository wants a javadoc jar beside the sources. The documentation is
// in the sources.
val emptyJavadocJar = tasks.register<org.gradle.jvm.tasks.Jar>("emptyJavadocJar") {
    archiveClassifier.set("javadoc")
}

afterEvaluate {
    publishing {
        publications {
            create<MavenPublication>("release") {
                from(components["release"])
                artifact(emptyJavadocJar)

                groupId = "com.offlineprotocol"
                artifactId = "offline-protocol-android"
                version = libraryVersion.get()

                pom {
                    name.set("Offline Protocol SDK for Android")
                    description.set(
                        "Offline-first messaging for Android: BLE mesh and internet relay " +
                            "transports, with MLS end-to-end encryption (RFC 9420) applied " +
                            "automatically."
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

        // Where the library is written as a Maven repository, whatever it is
        // later uploaded to. CI builds this on every pull request and checks
        // what is in it.
        repositories {
            maven {
                name = "staging"
                url = uri(layout.buildDirectory.dir("maven"))
            }
        }
    }

    // A signature is made only where a key was handed in, which is a
    // release. Asked for without one, every other build would fail at the
    // signing task.
    val signingKey = providers.gradleProperty("signingInMemoryKey")
    if (signingKey.isPresent) {
        signing {
            useInMemoryPgpKeys(
                signingKey.get(),
                providers.gradleProperty("signingInMemoryKeyPassword").orNull,
            )
            sign(publishing.publications["release"])
        }
    }
}
