plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
    id("org.jetbrains.kotlin.plugin.compose")
}

val embedModels = (project.findProperty("marginalia.embedModels") as String?) != "false"

android {
    namespace = "app.marginalia"
    compileSdk = 36
    ndkVersion = "30.0.16248370"

    defaultConfig {
        applicationId = "app.marginalia"
        minSdk = 28
        targetSdk = 36
        versionCode = 1
        versionName = "1.0"
        ndk { abiFilters += listOf("arm64-v8a", "x86_64") }
    }

    buildTypes {
        release {
            isMinifyEnabled = true
            isShrinkResources = true
            proguardFiles(getDefaultProguardFile("proguard-android-optimize.txt"), "proguard-rules.pro")
            signingConfig = signingConfigs.getByName("debug")
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    buildFeatures { compose = true; buildConfig = true }

    packaging {
        resources.excludes += "/META-INF/{AL2.0,LGPL2.1}"
        // The native library is large; keep it page-aligned and uncompressed so it maps straight from the APK.
        jniLibs { useLegacyPackaging = false }
    }
    // Models are already compressed GGUF/ggml files. Stored uncompressed they install faster
    // and are copied out at full speed on first launch.
    androidResources { noCompress += listOf("gguf", "bin", "litertlm") }

    sourceSets {
        getByName("main") {
            if (embedModels) assets.srcDir(rootProject.file("models-stage"))
        }
    }
}

kotlin {
    compilerOptions { jvmTarget.set(org.jetbrains.kotlin.gradle.dsl.JvmTarget.JVM_17) }
}

dependencies {
    val composeBom = platform("androidx.compose:compose-bom:2025.09.01")
    implementation(composeBom)
    implementation("androidx.compose.ui:ui")
    implementation("androidx.compose.ui:ui-graphics")
    implementation("androidx.compose.foundation:foundation")
    implementation("androidx.compose.animation:animation")
    implementation("androidx.compose.material3:material3")
    implementation("androidx.compose.material:material-icons-extended:1.7.8")
    implementation("androidx.compose.ui:ui-tooling-preview")
    debugImplementation("androidx.compose.ui:ui-tooling")

    implementation("androidx.core:core-ktx:1.17.0")
    implementation("androidx.core:core-splashscreen:1.0.1")
    implementation("androidx.activity:activity-compose:1.11.0")
    implementation("androidx.lifecycle:lifecycle-viewmodel-compose:2.9.4")
    implementation("androidx.lifecycle:lifecycle-runtime-compose:2.9.4")
    implementation("androidx.documentfile:documentfile:1.0.1")
    implementation("androidx.exifinterface:exifinterface:1.4.1")
    // Google LiteRT-LM, evaluated against llama.cpp (see scripts/bench.md)
    implementation("com.google.ai.edge.litertlm:litertlm-android:0.18.0")
    // Speaker separation (sherpa-onnx, Apache-2.0). Fetch the library with scripts/fetch-sherpa.sh.
    implementation(files("libs/sherpa-onnx-1.13.8.aar"))
    // Unpacks the small speaker model archive (bzip2).
    implementation("org.apache.commons:commons-compress:1.27.1")

    testImplementation("junit:junit:4.13.2")
    // The Android build has a stub of org.json that does nothing on the desktop JVM.
    testImplementation("org.json:json:20240303")
}
