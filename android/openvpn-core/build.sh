#!/usr/bin/env bash
# ics-openvpn(OpenVPN for Android)을 라이브러리(AAR)로 빌드해서 app/libs/openvpn-core.aar 를 만든다.
#
# 사용법:  ./build.sh [작업폴더]          (Windows Git Bash 기준)
#   작업폴더 기본값: /c/ics-openvpn-build  — CMake 오브젝트 경로 길이 제한 때문에 짧은 경로를 권장
#   OUT=<경로>   결과 AAR 위치 변경 (기본: ../app/libs/openvpn-core.aar)
#
# 필요한 것: git, JDK 17, swig, Android SDK(platforms;android-36, ndk;27.2.12479018, cmake;3.22.1)
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
WORK="${1:-/c/ics-openvpn-build}"
OUT="${OUT:-$HERE/../app/libs/openvpn-core.aar}"
UPSTREAM="https://github.com/schwabe/ics-openvpn.git"
COMMIT="660901ce320539aec476bd04bd1999c396d5e778"
SDK="${ANDROID_HOME:-${ANDROID_SDK_ROOT:-$HOME/AppData/Local/Android/Sdk}}"

command -v git  >/dev/null || { echo "git 이 필요합니다"; exit 1; }
command -v swig >/dev/null || { echo "swig 이 필요합니다 (winget install SWIG.SWIG 후 새 셸에서 실행)"; exit 1; }
[ -d "$SDK/ndk/27.2.12479018" ] || { echo "NDK 27.2.12479018 이 없습니다: sdkmanager \"ndk;27.2.12479018\" \"cmake;3.22.1\""; exit 1; }

mkdir -p "$WORK"
SRC="$WORK/src"

# 1) 고정 커밋으로 소스 + 서브모듈(openvpn, openssl, mbedtls, openvpn3, asio, lz4, fmt) 받기
if [ ! -d "$SRC/.git" ]; then
  git init -q "$SRC"
  git -C "$SRC" remote add origin "$UPSTREAM"
fi
git -C "$SRC" fetch -q --depth 1 origin "$COMMIT"
git -C "$SRC" checkout -q -f FETCH_HEAD
git -C "$SRC" submodule update --init --depth 1 --jobs 4

# 2) 라이브러리화 패치 적용 (이미 적용돼 있으면 건너뜀)
if git -C "$SRC" apply --check --ignore-whitespace "$HERE/ics-openvpn.patch" 2>/dev/null; then
  git -C "$SRC" apply --ignore-whitespace "$HERE/ics-openvpn.patch"
else
  echo "패치가 이미 적용돼 있거나 충돌합니다 — 적용된 상태로 계속합니다"
fi

# 3) Windows 에서는 git 심볼릭 링크가 일반 파일로 체크아웃되어 asio/asio/include 가 깨진다 → 실제 디렉터리로 교체
ASIO="$SRC/main/src/main/cpp/asio/asio"
if [ -f "$ASIO/include" ]; then
  rm "$ASIO/include"
  cp -r "$ASIO/../include" "$ASIO/include"
fi

# 4) SDK 위치 (Gradle 이 읽는 형식: 슬래시 경로)
echo "sdk.dir=$(cd "$SDK" && pwd -W 2>/dev/null || echo "$SDK")" | sed 's#\\#/#g' > "$SRC/local.properties"

# 5) 빌드 (arm64-v8a, OpenVPN 2.x 코어, UI 없는 skeleton 변형)
cd "$SRC"
if [ -x ./gradlew.bat ] && [[ "$(uname -s)" == MINGW* || "$(uname -s)" == MSYS* ]]; then
  ./gradlew.bat :main:assembleSkeletonOvpn2Release --console=plain
else
  ./gradlew :main:assembleSkeletonOvpn2Release --console=plain
fi

mkdir -p "$(dirname "$OUT")"
cp main/build/outputs/aar/main-skeleton-ovpn2-release.aar "$OUT"
echo "완료: $OUT"
