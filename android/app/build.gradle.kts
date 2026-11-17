plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
}

android {
    namespace = "com.kada.spike"
    compileSdk = 36

    defaultConfig {
        applicationId = "com.kada.spike"
        minSdk = 26
        targetSdk = 36
        versionCode = 9
        versionName = "0.2.0-s8"
    }

    buildTypes {
        debug {
            isMinifyEnabled = false
        }
        release {
            isMinifyEnabled = false
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    lint {
        // S0 鏄帰閽堝伐绋嬶紝涓嶅綋鎴愬彂甯冨寘瑕佹眰
        abortOnError = false
    }
}

kotlin {
    compilerOptions {
        jvmTarget.set(org.jetbrains.kotlin.gradle.dsl.JvmTarget.JVM_17)
    }
}
