# 내장 OpenVPN 코어 (ics-openvpn)

Android 앱이 VPN 터널을 직접 여는 데 쓰는 [ics-openvpn](https://github.com/schwabe/ics-openvpn)
(OpenVPN for Android) 코어입니다. `../app/libs/openvpn-core.aar` 로 앱에 들어갑니다.

> **라이선스: GPL v2** (ics-openvpn 및 그 서브모듈의 라이선스를 따릅니다).
> 이 AAR 을 포함한 앱을 **배포**하면 앱 소스도 GPL 로 공개해야 합니다. 개인용으로만 쓰는 한 문제 없습니다.

## 구성

| 파일 | 설명 |
|---|---|
| `../app/libs/openvpn-core.aar` | 미리 빌드된 결과물 (arm64-v8a, 약 8MB). 앱 빌드는 이것만 있으면 됩니다. |
| `ics-openvpn.patch` | upstream 을 라이브러리로 쓰기 위한 수정 (4개 파일) |
| `build.sh` | upstream 을 고정 커밋으로 받아 패치하고 AAR 을 다시 만드는 스크립트 |

upstream 고정 커밋: `660901ce320539aec476bd04bd1999c396d5e778`

## 왜 패치가 필요한가

upstream README 에 *"NOT about creating a library to be used in other projects"* 라고 적혀 있어서,
앱(application)으로만 빌드되도록 되어 있습니다. `ics-openvpn.patch` 가 하는 일:

- `com.android.application` → `com.android.library` 로 전환, 서명/분할/번들 설정 제거
- `compileSdk` 37 → 36, NDK 30 → 27.2.12479018, `androidx.core` 1.19.0 → 1.16.0 (설치된 SDK 에 맞춤)
- `arm64-v8a` 만 빌드
- **`-DANDROID_SUPPORT_FLEXIBLE_PAGE_SIZES=ON`** — 네이티브 라이브러리를 16KB 페이지 정렬로 링크
  (NDK 27 기본은 4KB 라서, 16KB 기기에서 "ELF 정렬 검사 실패" 경고가 뜸)
- 데모용 모듈(`tlsexternalcertprovider`, `remoteExample`) 제외, swig 경로 지정

`build.sh` 는 추가로 Windows 에서 깨지는 `asio/asio/include` 심볼릭 링크(git 이 일반 파일로 체크아웃함)를
실제 디렉터리로 바꿔 줍니다.

## 다시 빌드하기

```bash
# 필요: git, JDK 17, swig(winget install SWIG.SWIG), Android SDK 의 NDK 27.2.12479018 / cmake 3.22.1
./build.sh                 # 기본 작업폴더 /c/ics-openvpn-build, 결과는 ../app/libs/openvpn-core.aar
./build.sh /c/icsb         # 작업폴더 지정
```

## 앱 쪽에서 필요한 설정 (이미 적용됨)

- `app/build.gradle.kts`
  - `packaging { jniLibs { useLegacyPackaging = true } }` — OpenVPN 2.x 가 `libovpnexec.so` 를 별도 프로세스로
    실행하므로 네이티브 라이브러리가 디스크에 풀려 있어야 합니다.
  - `-Xskip-metadata-version-check` — AAR 이 더 새로운 Kotlin 으로 빌드됨
  - `ndk { abiFilters += "arm64-v8a" }`
- `AndroidManifest.xml`
  - `RECEIVE_BOOT_COMPLETED` 는 **유지해야 합니다.** 코어의 `keepVPNAlive` 가 persisted job 을 등록하는데, 이 권한이
    없으면 `:openvpn` 프로세스가 죽습니다.
  - 코어가 외부 앱에 노출하는 컴포넌트(원격 제어 API, 부팅 자동 시작, 외부 실행용 액티비티)는 `tools:node="remove"` 로 제거
- `proguard-rules.pro` — `de.blinkt.openvpn.**` 유지 (JNI/AIDL/리플렉션 참조)
