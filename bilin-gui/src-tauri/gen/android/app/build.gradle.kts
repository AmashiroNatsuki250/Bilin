import java.util.Properties
import groovy.json.JsonSlurper

plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
    id("rust")
}

val tauriProperties = Properties().apply {
    val propFile = file("tauri.properties")
    if (propFile.exists()) {
        propFile.inputStream().use { load(it) }
    }
}

val versionPattern = Regex("""^(\d+)\.(\d+)\.(\d+)$""")

val tauriVersionName = tauriProperties.getProperty("tauri.android.versionName")?.ifBlank { null } ?: run {
    val tauriConfFile = file("../../../tauri.conf.json")
    check(tauriConfFile.exists()) { "Missing tauri.conf.json at ${tauriConfFile.path}" }

    val tauriConf = tauriConfFile.reader(Charsets.UTF_8).use { JsonSlurper().parse(it) as? Map<*, *> }
        ?: error("Failed to parse ${tauriConfFile.path} as a JSON object")
    tauriConf["version"] as? String
        ?: error("Missing string field \"version\" in ${tauriConfFile.path}")
}

val tauriVersionMatch = versionPattern.matchEntire(tauriVersionName)
    ?: error("Android version must use x.y.z format, but got \"$tauriVersionName\"")

val tauriVersionCode = if (tauriProperties.getProperty("tauri.android.versionName")?.ifBlank { null } != null) {
    val versionCodeProp = tauriProperties.getProperty("tauri.android.versionCode")
    if (versionCodeProp != null) {
        versionCodeProp.toIntOrNull()
            ?: error("Property \"tauri.android.versionCode\" must be an integer, but got \"$versionCodeProp\"")
    } else {
        val (major, minor, patch) = tauriVersionMatch.destructured
        major.toInt() * 1_000_000 + minor.toInt() * 1_000 + patch.toInt()
    }
} else {
    val (major, minor, patch) = tauriVersionMatch.destructured
    major.toInt() * 1_000_000 + minor.toInt() * 1_000 + patch.toInt()
}

val releaseKeystore = providers.environmentVariable("BILIN_ANDROID_KEYSTORE").orNull
val localSigningFile = rootProject.file("keystore.properties")
val localSigningProperties = Properties().apply {
    if (localSigningFile.isFile) localSigningFile.inputStream().use { load(it) }
}
val useReleaseSigning = releaseKeystore != null || localSigningFile.isFile
val signPreview = providers.environmentVariable("BILIN_ANDROID_SIGN_PREVIEW").orNull == "true"

fun signingProperty(property: String, environment: String): String {
    val value = if (releaseKeystore != null) providers.environmentVariable(environment).orNull
        else localSigningProperties.getProperty(property)
    return value?.takeIf { it.isNotBlank() } ?: error("Missing Android release signing setting: $property")
}

android {
    compileSdk = 34
    namespace = "io.github.amashironatsuki250.bilin"
    defaultConfig {
        manifestPlaceholders["usesCleartextTraffic"] = "false"
        applicationId = "io.github.amashironatsuki250.bilin"
        minSdk = 24
        targetSdk = 34
        versionCode = tauriVersionCode
        versionName = tauriVersionName
    }
    if (useReleaseSigning) {
        signingConfigs.create("release") {
            keyAlias = signingProperty("keyAlias", "BILIN_ANDROID_KEY_ALIAS")
            keyPassword = signingProperty("keyPassword", "BILIN_ANDROID_KEY_PASSWORD")
            storeFile = file(releaseKeystore ?: signingProperty("storeFile", "BILIN_ANDROID_KEYSTORE"))
            storePassword = signingProperty("storePassword", "BILIN_ANDROID_STORE_PASSWORD")
        }
    }
    buildTypes {
        getByName("debug") {
            manifestPlaceholders["usesCleartextTraffic"] = "true"
            isDebuggable = true
            isJniDebuggable = true
            isMinifyEnabled = false
            packaging {                jniLibs.keepDebugSymbols.add("*/arm64-v8a/*.so")
                jniLibs.keepDebugSymbols.add("*/armeabi-v7a/*.so")
                jniLibs.keepDebugSymbols.add("*/x86/*.so")
                jniLibs.keepDebugSymbols.add("*/x86_64/*.so")
            }
        }
        getByName("release") {
            isMinifyEnabled = true
            proguardFiles(
                *fileTree(".") { include("**/*.pro") }
                    .plus(getDefaultProguardFile("proguard-android-optimize.txt"))
                    .toList().toTypedArray()
            )
            if (useReleaseSigning) {
                signingConfig = signingConfigs.getByName("release")
            } else if (signPreview) {
                applicationIdSuffix = ".preview"
                signingConfig = signingConfigs.getByName("debug")
            }
        }
    }
    kotlinOptions {
        jvmTarget = "1.8"
    }
    buildFeatures {
        buildConfig = true
    }
}

rust {
    rootDirRel = "../../../"
}

dependencies {
    implementation("androidx.webkit:webkit:1.6.1")
    implementation("androidx.appcompat:appcompat:1.6.1")
    implementation("com.google.android.material:material:1.8.0")
    testImplementation("junit:junit:4.13.2")
    androidTestImplementation("androidx.test.ext:junit:1.1.4")
    androidTestImplementation("androidx.test.espresso:espresso-core:3.5.0")
}

apply(from = "tauri.build.gradle.kts")
