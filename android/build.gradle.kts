// S0 spike：只用本机 Gradle 缓存里已有的 AGP / Kotlin 插件。
// 走 buildscript classpath 而不是 `plugins { id(...) version ... }`：插件 marker 构件
// 不在缓存里，离线解析会失败（真机没网时也编不了）。
buildscript {
    repositories {
        google()
        mavenCentral()
    }
    dependencies {
        // 本机离线缓存里只有 AGP 8.13.0 的 aapt2（8.13.2 的 aapt2 未缓存）。
        classpath("com.android.tools.build:gradle:8.13.0")
        classpath("org.jetbrains.kotlin:kotlin-gradle-plugin:2.2.20")
        // 本机离线构建：缓存里只有 kotlin-reflect 2.2.0，而 AGP 的依赖链会按 2.0.21 拉取。
        // 显式抬到 2.2.0（Gradle 取最高版本）。联网环境同样无害。
        classpath("org.jetbrains.kotlin:kotlin-reflect:2.2.0")
    }
}

allprojects {
    repositories {
        google()
        mavenCentral()
    }
}

// 本机离线构建：缓存里只有 kotlin-reflect 2.2.0（被 AGP 的依赖链按 2.0.21 拉取），
// 已在上面的 buildscript classpath 里显式抬高版本。

