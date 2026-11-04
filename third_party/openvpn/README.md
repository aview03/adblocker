# OpenVPN (공식 Windows 설치 패키지)

Windows 앱이 `openvpn.exe` 가 없을 때 첫 접속 시 자동으로 설치하는 패키지입니다
(`src/vpn.rs` 의 `install_openvpn`, 빌드 시 `include_bytes!` 로 exe 안에 포함됨).

| 항목 | 값 |
|---|---|
| 파일 | `OpenVPN-2.7.8-I002-amd64.msi` |
| 출처 | https://build.openvpn.net/downloads/releases/OpenVPN-2.7.8-I002-amd64.msi |
| SHA-256 | `33104dd02c48cba172612882b1af6276422b1b4340d4c9c84dd2afb4b356fe9f` |
| 서명 | Authenticode 유효, 서명자 `OpenVPN Inc.` |
| 라이선스 | GPL v2 (OpenSSL 예외 포함). 배포하면 소스 공개 의무가 있음 — 개인용이면 해당 없음 |

설치되는 기능은 `OpenVPN`(코어), `Drivers`, `Drivers.TAPWindows6` 뿐입니다
(`msiexec /i ... /qn ADDLOCAL=OpenVPN,Drivers,Drivers.TAPWindows6`).
VPN Gate 서버는 CBC 암호를 쓰므로 DCO 드라이버는 사용할 수 없어 설치하지 않습니다.
OpenVPN 2.7 은 Wintun 지원을 제거했으므로(`--windows-driver` 는 폐기됨) TAP-Windows6 를 쓰고, 앱은 `--disable-dco` 로 실행합니다.

## 업데이트하기
1. https://build.openvpn.net/downloads/releases/ 에서 새 `*-amd64.msi` 를 받고
   `Get-AuthenticodeSignature` 로 서명을 확인한다.
2. 이 폴더의 MSI 를 교체하고 위 표와 `src/vpn.rs` 의 `include_bytes!` 경로를 갱신한다.
