plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
    id("org.jetbrains.kotlin.plugin.compose")
}

android {
    namespace = "com.thinkgood.adblocker"
    compileSdk = 36

    defaultConfig {
        applicationId = "com.thinkgood.adblocker"
        minSdk = 30
        targetSdk = 36
        versionCode = 1
        versionName = "0.1.0"
        // 내장 OpenVPN 코어는 arm64 만 포함
        ndk { abiFilters += "arm64-v8a" }
    }

    buildTypes {
        release {
            isMinifyEnabled = true
            isShrinkResources = true
            proguardFiles(getDefaultProguardFile("proguard-android-optimize.txt"), "proguard-rules.pro")
            // 개인용: 디버그 키로 서명해 바로 설치 가능하게 함
            signingConfig = signingConfigs.getByName("debug")
        }
    }
    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    kotlinOptions {
        jvmTarget = "17"
        // 내장 OpenVPN 코어 AAR 이 더 새로운 Kotlin 으로 빌드됨 (우리는 그 중 Java 클래스만 사용)
        freeCompilerArgs += "-Xskip-metadata-version-check"
    }
    buildFeatures { compose = true }
    packaging {
        // OpenVPN 2.x 코어는 libovpnexec.so 를 별도 프로세스로 exec 하므로
        // 네이티브 라이브러리가 APK 안이 아니라 디스크에 풀려 있어야 한다.
        jniLibs { useLegacyPackaging = true }
    }
}

dependencies {
    // ics-openvpn 코어(GPLv2) — android/openvpn-core/README.md 참고
    implementation(files("libs/openvpn-core.aar"))
    implementation("androidx.annotation:annotation:1.9.1")
    implementation("androidx.core:core-ktx:1.16.0")
    implementation(platform("androidx.compose:compose-bom:2025.10.00"))
    implementation("androidx.compose.ui:ui")
    implementation("androidx.compose.foundation:foundation")
    implementation("androidx.compose.material3:material3")
    implementation("androidx.activity:activity-compose:1.11.0")
    implementation("androidx.lifecycle:lifecycle-runtime-compose:2.9.4")
}
