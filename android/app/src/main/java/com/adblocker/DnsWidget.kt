package com.adblocker

import android.app.PendingIntent
import android.appwidget.AppWidgetManager
import android.appwidget.AppWidgetProvider
import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.graphics.Color
import android.widget.RemoteViews

/**
 * 홈 화면 위젯 (가로 2칸·4칸·5칸 x 세로 1칸).
 * 탭하면 DNS 보호를 켜고/끈다. 권한이 없거나 VPN 사용 중이면 앱을 연다.
 */
abstract class DnsWidget(private val layout: Int, private val compact: Boolean) : AppWidgetProvider() {

    override fun onUpdate(ctx: Context, mgr: AppWidgetManager, ids: IntArray) {
        for (id in ids) mgr.updateAppWidget(id, build(ctx))
    }

    override fun onReceive(ctx: Context, intent: Intent) {
        if (intent.action == ACTION_TOGGLE) {
            if (DnsController.hasPermission(ctx) && !VpnController.isActive(ctx)) {
                DnsController.setEnabled(ctx, !DnsController.isEnabled(ctx))
            }
            updateAll(ctx)
            return
        }
        super.onReceive(ctx, intent)
    }

    private fun build(ctx: Context): RemoteViews {
        val vpn = VpnController.isActive(ctx)
        val on = DnsController.isEnabled(ctx) && !vpn
        val lit = on || vpn
        val p = DnsController.active(ctx) ?: DnsController.selected(ctx)
        val v = RemoteViews(ctx.packageName, layout)

        val fg = if (lit) Color.WHITE else Color.BLACK
        val sub = if (lit) 0xCCFFFFFF.toInt() else 0xFF666666.toInt()
        v.setInt(R.id.widget_root, "setBackgroundResource", if (lit) R.drawable.widget_bg_on else R.drawable.widget_bg_off)
        v.setTextViewText(R.id.widget_title, if (compact) p.label else "${p.label} DNS")
        v.setTextColor(R.id.widget_title, fg)
        v.setTextViewText(R.id.widget_status, if (vpn) "VPN 사용중" else if (on) "보호 중" else "꺼짐")
        v.setTextColor(R.id.widget_status, sub)

        // 원형 ON/OFF 버튼: 켜짐 = 흰 원 + 빨간 글씨, 꺼짐 = 빨간 원 + 흰 글씨
        v.setTextViewText(R.id.widget_action, if (lit) "ON" else "OFF")
        v.setTextColor(R.id.widget_action, if (lit) 0xFFEC1B1B.toInt() else Color.WHITE)
        v.setInt(R.id.widget_action, "setBackgroundResource", if (lit) R.drawable.widget_pill_light else R.drawable.widget_pill_red)

        val open = PendingIntent.getActivity(
            ctx, 0, Intent(ctx, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        val toggle = PendingIntent.getBroadcast(
            ctx, javaClass.hashCode(), Intent(ctx, javaClass).setAction(ACTION_TOGGLE),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        val canToggle = DnsController.hasPermission(ctx) && !vpn
        v.setOnClickPendingIntent(R.id.widget_root, if (canToggle) toggle else open)
        return v
    }

    companion object {
        const val ACTION_TOGGLE = "com.adblocker.WIDGET_TOGGLE"

        /** 상태가 바뀔 때 모든 위젯을 다시 그린다. */
        fun updateAll(ctx: Context) {
            val app = ctx.applicationContext
            val mgr = AppWidgetManager.getInstance(app)
            for (cls in listOf(DnsWidget2x1::class.java, DnsWidget4x1::class.java, DnsWidget5x1::class.java)) {
                val ids = mgr.getAppWidgetIds(ComponentName(app, cls))
                if (ids.isNotEmpty()) {
                    app.sendBroadcast(
                        Intent(app, cls).setAction(AppWidgetManager.ACTION_APPWIDGET_UPDATE)
                            .putExtra(AppWidgetManager.EXTRA_APPWIDGET_IDS, ids),
                    )
                }
            }
        }
    }
}

class DnsWidget2x1 : DnsWidget(R.layout.widget_2x1, compact = true)
class DnsWidget4x1 : DnsWidget(R.layout.widget_4x1, compact = false)
class DnsWidget5x1 : DnsWidget(R.layout.widget_5x1, compact = false)
