package com.thinkgood.adblocker

import android.app.PendingIntent
import android.content.Intent
import android.os.Build
import android.service.quicksettings.Tile
import android.service.quicksettings.TileService

/** 빠른 설정(알림창 상단) 타일: 탭하면 AdGuard DNS 켜기/끄기. */
class DnsTileService : TileService() {

    override fun onStartListening() = refresh()

    override fun onClick() {
        if (!DnsController.hasPermission(this)) {
            openApp()
            return
        }
        DnsController.setEnabled(this, !DnsController.isEnabled(this))
        refresh()
    }

    private fun refresh() {
        val tile = qsTile ?: return
        val on = DnsController.isEnabled(this)
        tile.state = if (on) Tile.STATE_ACTIVE else Tile.STATE_INACTIVE
        val p = DnsController.active(this) ?: DnsController.selected(this)
        tile.label = "${p.label} DNS"
        tile.subtitle = if (on) "보호 중" else "꺼짐"
        tile.updateTile()
    }

    @Suppress("DEPRECATION")
    private fun openApp() {
        val intent = Intent(this, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
        if (Build.VERSION.SDK_INT >= 34) {
            startActivityAndCollapse(
                PendingIntent.getActivity(this, 0, intent, PendingIntent.FLAG_IMMUTABLE)
            )
        } else {
            startActivityAndCollapse(intent)
        }
    }
}
