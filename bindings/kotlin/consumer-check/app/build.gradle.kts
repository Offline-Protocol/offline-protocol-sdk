plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
}

val libraryVersion = providers.gradleProperty("VERSION_NAME").orElse("0.0.0-local")

// The SDK levels here are this application's own. An application chooses
// them, and the library has to build into one that chose its floor.
android {
    namespace = "com.offlineprotocol.consumercheck"
    compileSdk = 34

    defaultConfig {
        applicationId = "com.offlineprotocol.consumercheck"
        minSdk = 24
        targetSdk = 34
        versionCode = 1
        versionName = "1"
    }

    buildTypes {
        // Minified, because that is where a library fails an application
        // that did nothing wrong: R8 stops on a class it cannot find, and
        // the rules that tell it which ones do not matter have to arrive
        // with the library. No rule is written here.
        release {
            isMinifyEnabled = true
            proguardFiles(getDefaultProguardFile("proguard-android-optimize.txt"))
            signingConfig = signingConfigs.getByName("debug")
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    kotlinOptions {
        jvmTarget = "17"
    }
}

// Manual jniLibs copies must use libuniffi_offline_protocol.so, not the cargo
// output name. See docs/android-jni-and-imports.md and OfflineProtocolJniLibrary.
tasks.register("checkJniLibraryNames") {
    val jniRoot = file("src/main/jniLibs")
    doLast {
        if (!jniRoot.isDirectory) {
            return@doLast
        }
        jniRoot.walk().filter { it.isFile && it.extension == "so" }.forEach { so ->
            when (so.name) {
                "libuniffi_offline_protocol.so" -> Unit
                "liboffline_protocol_uniffi.so" -> error(
                    "${so}: rename to libuniffi_offline_protocol.so (see OfflineProtocolJniLibrary)"
                )
                else -> error("${so}: unexpected native library name for manual jniLibs")
            }
        }
    }
}
tasks.named("preBuild") { dependsOn("checkJniLibraryNames") }

dependencies {
    // The one line the README gives an application. Nothing else is declared
    // here, JNA included: if this stops compiling, the library's public API
    // has started to need something its metadata does not bring.
    implementation("com.offlineprotocol:offline-protocol-sdk:${libraryVersion.get()}")
}
