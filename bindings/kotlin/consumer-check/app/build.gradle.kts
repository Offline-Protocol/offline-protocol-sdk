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

dependencies {
    // The one line the README gives an application. Nothing else is declared
    // here, JNA included: if this stops compiling, the library's public API
    // has started to need something its metadata does not bring.
    implementation("com.offlineprotocol:offline-protocol-android:${libraryVersion.get()}")
}
