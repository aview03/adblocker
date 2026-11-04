package com.thinkgood.adblocker

import android.content.Context
import android.content.pm.PackageManager
import android.provider.Settings

enum class Provider(
    val label: String,
    val note: String,
    val hostname: String,
    val v4: List<String>,
    val v6: List<String>,
) {
    ADGUARD(
        "AdGuard", "광고·추적 차단", "dns.adguard-dns.com",
        listOf("94.140.14.14", "94.140.15.15"), listOf("2a10:50c0::ad1:ff", "2a10:50c0::ad2:ff"),
    ),
    CLOUDFLARE(
        "Cloudflare", "1.1.1.1 · 빠른 속도", "one.one.one.one",
        listOf("1.1.1.1", "1.0.0.1"), listOf("2606:4700:4700::1111", "2606:4700:4700::1001"),
    ),
    GOOGLE(
        "Google", "8.8.8.8 · 안정성", "dns.google",
        listOf("8.8.8.8", "8.8.4.4"), listOf("2001:4860:4860::8888", "2001:4860:4860::8844"),
    );
}

/** Android 의 "비공개 DNS(Private DNS)" 설정을 직접 바꾼다. */
object DnsController {
    const val PACKAGE = "com.thinkgood.adblocker"
    const val PERMISSION = "android.permission.WRITE_SECURE_SETTINGS"
    const val ADB_COMMAND = "adb shell pm grant $PACKAGE $PERMISSION"

    private const val KEY_MODE = "private_dns_mode"
    private const val KEY_SPECIFIER = "private_dns_specifier"
    private const val PREFS = "prefs"
    private const val PREF_PROVIDER = "provider"

    fun hasPermission(ctx: Context): Boolean =
        ctx.checkSelfPermission(PERMISSION) == PackageManager.PERMISSION_GRANTED

    /** 사용자가 고른 제공자 (기본값 AdGuard). */
    fun selected(ctx: Context): Provider {
        val name = ctx.getSharedPreferences(PREFS, Context.MODE_PRIVATE).getString(PREF_PROVIDER, null)
        return Provider.entries.firstOrNull { it.name == name } ?: Provider.ADGUARD
    }

    fun select(ctx: Context, p: Provider) {
        ctx.getSharedPreferences(PREFS, Context.MODE_PRIVATE).edit().putString(PREF_PROVIDER, p.name).apply()
    }

    /** 지금 시스템에 적용되어 있는 제공자. 우리가 관리하는 호스트가 아니면 null (꺼짐 취급). */
    fun active(ctx: Context): Provider? {
        val r = ctx.contentResolver
        if (Settings.Global.getString(r, KEY_MODE) != "hostname") return null
        val host = Settings.Global.getString(r, KEY_SPECIFIER)
        return Provider.entries.firstOrNull { it.hostname == host }
    }

    fun isEnabled(ctx: Context): Boolean = active(ctx) != null

    /** 켜기: 비공개 DNS = 선택한 제공자 호스트명. 끄기: 기본값(자동)으로 복구. */
    fun setEnabled(ctx: Context, enable: Boolean): Result<Boolean> = runCatching {
        if (!hasPermission(ctx)) error("WRITE_SECURE_SETTINGS 권한이 없습니다")
        val r = ctx.contentResolver
        val ok = if (enable) {
            Settings.Global.putString(r, KEY_SPECIFIER, selected(ctx).hostname) &&
                Settings.Global.putString(r, KEY_MODE, "hostname")
        } else {
            Settings.Global.putString(r, KEY_MODE, "opportunistic")
        }
        if (!ok) error("설정을 저장하지 못했습니다")
        enable
    }
}
