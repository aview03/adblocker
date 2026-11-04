# 내장 OpenVPN 코어: JNI / AIDL / 리플렉션으로 이름이 참조된다
-keep class de.blinkt.openvpn.** { *; }
-keep class net.openvpn.ovpn3.** { *; }
-dontwarn de.blinkt.openvpn.**
