package com.adblocker

import android.content.pm.ApplicationInfo
import android.util.Log
import de.blinkt.openvpn.core.ICSOpenVPNApplication
import de.blinkt.openvpn.core.VpnStatus

/** OpenVPN 코어(알림 채널, 상태 수신)를 초기화하기 위해 ICSOpenVPNApplication 을 그대로 쓴다. */
class App : ICSOpenVPNApplication() {
    override fun onCreate() {
        super.onCreate()
        // 디버그 빌드에서만: 접속 실패 원인 확인용으로 엔진 로그를 logcat(VPNCORE)으로 내보낸다
        if (applicationInfo.flags and ApplicationInfo.FLAG_DEBUGGABLE != 0) {
            VpnStatus.addLogListener { item -> Log.d("VPNCORE", item.getString(this)) }
        }
    }
}
