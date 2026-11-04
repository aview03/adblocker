package com.thinkgood.adblocker

import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.tween
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.foundation.verticalScroll
import androidx.compose.foundation.rememberScrollState
import androidx.compose.material3.Text
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.drawscope.DrawScope
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.lerp
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.compose.LifecycleEventEffect
import kotlin.math.*

private val Red = Color(0xFFEC1B1B)
private val Black = Color.Black
private val White = Color.White
private val Gray = Color(0xFF707070)
private val Line = Color(0xFFE4E4E4)
private val Corner = RoundedCornerShape(2.dp)

class MainActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        setContent { Screen() }
    }
}

@Composable
private fun Screen() {
    val ctx = LocalContext.current
    var dnsOn by remember { mutableStateOf(DnsController.isEnabled(ctx)) }
    var provider by remember { mutableStateOf(DnsController.active(ctx) ?: DnsController.selected(ctx)) }
    var granted by remember { mutableStateOf(DnsController.hasPermission(ctx)) }
    var error by remember { mutableStateOf<String?>(null) }
    var copied by remember { mutableStateOf(false) }

    // 설정/타일에서 바뀐 값을 돌아올 때 다시 읽음
    LifecycleEventEffect(Lifecycle.Event.ON_RESUME) {
        dnsOn = DnsController.isEnabled(ctx)
        provider = DnsController.active(ctx) ?: DnsController.selected(ctx)
        granted = DnsController.hasPermission(ctx)
    }

    Column(
        Modifier
            .fillMaxSize()
            .background(White)
            .statusBarsPadding()
            .navigationBarsPadding()
            .verticalScroll(rememberScrollState())
            .padding(24.dp)
    ) {
        Text("${provider.label} DNS", fontSize = 18.sp, fontWeight = FontWeight.Bold, color = Black)
        Spacer(Modifier.height(20.dp))

        // 상태 + 오브
        Row(Modifier.fillMaxWidth().height(84.dp), verticalAlignment = Alignment.Top) {
            Column(Modifier.weight(1f)) {
                Text("상태", fontSize = 12.sp, color = Gray)
                Text(
                    if (dnsOn) "보호 중" else "꺼짐",
                    fontSize = 30.sp,
                    color = if (dnsOn) Red else Gray,
                )
            }
            Orb(dnsOn, Modifier.size(120.dp, 84.dp))
        }
        Spacer(Modifier.height(16.dp))

        // 메인 버튼
        Box(
            Modifier
                .fillMaxWidth()
                .height(52.dp)
                .clip(Corner)
                .background(if (!granted) Gray else if (dnsOn) Black else Red)
                .clickable(enabled = granted) {
                    DnsController.setEnabled(ctx, !dnsOn)
                        .onSuccess { dnsOn = it; error = null }
                        .onFailure { error = it.message }
                },
            contentAlignment = Alignment.Center,
        ) {
            Text(if (dnsOn) "끄기" else "켜기", color = White, fontSize = 16.sp, fontWeight = FontWeight.Bold)
        }
        error?.let {
            Spacer(Modifier.height(8.dp))
            Text(it, fontSize = 11.sp, color = Red)
        }

        // 권한 안내
        if (!granted) {
            Spacer(Modifier.height(16.dp))
            Column(
                Modifier.fillMaxWidth().border(1.dp, Red, Corner).padding(14.dp)
            ) {
                Text("권한이 필요합니다", fontSize = 14.sp, fontWeight = FontWeight.Bold, color = Red)
                Spacer(Modifier.height(6.dp))
                Text(
                    "비공개 DNS 설정을 바꾸려면 PC에서 USB 디버깅으로 한 번만 아래 명령을 실행하세요.",
                    fontSize = 12.sp, color = Black,
                )
                Spacer(Modifier.height(8.dp))
                SelectionContainer {
                    Text(
                        DnsController.ADB_COMMAND,
                        fontSize = 11.sp, fontFamily = FontFamily.Monospace, color = Black,
                        modifier = Modifier.fillMaxWidth().background(Line).padding(8.dp),
                    )
                }
                Spacer(Modifier.height(8.dp))
                OutlineButton(if (copied) "복사됨" else "명령 복사") {
                    val cm = ctx.getSystemService(Context.CLIPBOARD_SERVICE) as ClipboardManager
                    cm.setPrimaryClip(ClipData.newPlainText("adb", DnsController.ADB_COMMAND))
                    copied = true
                }
            }
        }

        Spacer(Modifier.height(20.dp))
        Box(Modifier.fillMaxWidth().height(1.dp).background(Line))
        Spacer(Modifier.height(12.dp))

        Text("DNS 제공자", fontSize = 12.sp, color = Gray)
        Spacer(Modifier.height(6.dp))
        Provider.entries.forEach { p ->
            ProviderRow(p, selected = p == provider) {
                provider = p
                DnsController.select(ctx, p)
                // 이미 켜져 있으면 즉시 새 제공자로 전환
                if (dnsOn && granted) {
                    DnsController.setEnabled(ctx, true).onFailure { error = it.message }
                }
            }
        }

        Spacer(Modifier.height(12.dp))
        Box(Modifier.fillMaxWidth().height(1.dp).background(Line))
        Spacer(Modifier.height(12.dp))

        Text(
            "빠른 설정 패널(알림창을 내려 편집)에 \"AdGuard DNS\" 타일을 추가하면 앱을 열지 않고도 켜고 끌 수 있습니다. " +
                "설정은 재부팅 후에도 유지되므로 별도의 자동 시작 등록은 필요 없습니다.",
            fontSize = 12.sp, color = Gray,
        )

        Spacer(Modifier.height(12.dp))
        Box(Modifier.fillMaxWidth().height(1.dp).background(Line))
        Spacer(Modifier.height(12.dp))

        Text("서버", fontSize = 12.sp, color = Gray)
        Text(provider.hostname, fontSize = 12.sp, color = Black)
        (provider.v4 + provider.v6).forEach {
            Text(it, fontSize = 12.sp, fontFamily = FontFamily.Monospace, color = Black)
        }
    }
}

@Composable
private fun ProviderRow(p: Provider, selected: Boolean, onClick: () -> Unit) {
    Row(
        Modifier
            .fillMaxWidth()
            .padding(vertical = 3.dp)
            .clip(Corner)
            .border(1.dp, if (selected) Red else Line, Corner)
            .clickable(onClick = onClick)
            .padding(horizontal = 14.dp, vertical = 10.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Column(Modifier.weight(1f)) {
            Text(p.label, fontSize = 14.sp, fontWeight = FontWeight.Bold, color = Black)
            Text(p.note, fontSize = 11.sp, color = Gray)
        }
        Box(
            Modifier.size(18.dp).border(2.dp, if (selected) Red else Line, RoundedCornerShape(9.dp)),
            contentAlignment = Alignment.Center,
        ) {
            if (selected) Box(Modifier.size(8.dp).background(Red, RoundedCornerShape(4.dp)))
        }
    }
}

@Composable
private fun OutlineButton(text: String, onClick: () -> Unit) {
    Box(
        Modifier
            .fillMaxWidth()
            .height(40.dp)
            .clip(Corner)
            .border(1.dp, Black, Corner)
            .clickable(onClick = onClick),
        contentAlignment = Alignment.Center,
    ) { Text(text, color = Black, fontSize = 14.sp) }
}

// ---------- 오브 (Windows 버전 draw_orb 이식) ----------

@Composable
private fun Orb(on: Boolean, modifier: Modifier) {
    val k by animateFloatAsState(if (on) 1f else 0f, tween(900), label = "orb_k")
    var t by remember { mutableFloatStateOf(0f) }
    var phase by remember { mutableFloatStateOf(0f) }
    val kNow by rememberUpdatedState(k)
    LaunchedEffect(Unit) {
        var last = 0L
        while (true) {
            withFrameNanos { now ->
                if (last != 0L) {
                    val dt = (now - last) / 1e9f
                    t += dt
                    phase += dt * kNow
                }
                last = now
            }
        }
    }
    Canvas(modifier) {
        val d = density
        drawOrb(Offset(size.width - 46f * d, size.height / 2f), d, t, phase, k)
    }
}

private fun rot3(v: FloatArray, roll: Float, yaw: Float, tilt: Float): FloatArray {
    var (x, y, z) = v.toList()
    var nx = x * cos(roll) - y * sin(roll)
    var ny = x * sin(roll) + y * cos(roll)
    x = nx; y = ny
    nx = x * cos(yaw) + z * sin(yaw)
    z = -x * sin(yaw) + z * cos(yaw)
    x = nx
    ny = y * cos(tilt) - z * sin(tilt)
    z = y * sin(tilt) + z * cos(tilt)
    y = ny
    return floatArrayOf(x, y, z)
}

private fun Color.a(m: Float) = copy(alpha = (alpha * m).coerceIn(0f, 1f))

private fun DrawScope.drawOrb(base: Offset, d: Float, t: Float, phase: Float, k: Float) {
    val bob = sin(t * 1.5f)
    val c = base + Offset(0f, bob * 3.5f * d)
    val r = 22f * d
    val col = lerp(Color(0xFF55555C), Red, k)
    val pulse = sin(phase * 2.4f) * 0.5f + 0.5f
    fun at(v: FloatArray, rad: Float) = Offset(c.x + v[0] * rad, c.y + v[1] * rad)

    // 그림자
    val sh = 1f - bob * 0.12f
    drawOval(
        Color.Black.copy(alpha = 0.09f),
        topLeft = base + Offset(-r * 0.8f * sh, 40f * d - 3f * d * sh),
        size = Size(r * 1.6f * sh, 6f * d * sh),
    )

    // 오라
    for (i in 0 until 6) {
        val rr = r + (2f + i * 4.5f + pulse * 4f * k) * d
        val a = (0.04f + 0.15f * k) * (1f - i / 6f)
        drawCircle(col.a(a), rr, c)
    }

    // 궤도 링
    val rings = listOf(Triple(0.35f, 1.25f, 1f), Triple(-0.9f, 1.0f, -1f))
    val ringR = r * 1.55f
    val ringAlpha = 0.18f + 0.55f * k
    fun drawRing(front: Boolean) {
        for ((roll, tilt, dir) in rings) {
            val spin = phase * 0.6f * dir
            val pts = (0..72).map { n ->
                val a = n / 72f * (2 * PI.toFloat())
                rot3(floatArrayOf(cos(a), 0f, sin(a)), roll, spin, tilt)
            }
            for (i in 0 until pts.size - 1) {
                val z = (pts[i][2] + pts[i + 1][2]) * 0.5f
                if ((z >= 0f) != front) continue
                val depth = 0.45f + 0.55f * min(abs(z), 1f) * (if (front) 1f else 0.4f)
                drawLine(
                    col.a(ringAlpha * depth), at(pts[i], ringR), at(pts[i + 1], ringR),
                    strokeWidth = (if (front) 1.4f else 0.9f) * d,
                )
            }
        }
    }
    drawRing(false)

    // 구체
    val dark = lerp(Color.Black, col, 0.40f)
    val bright = lerp(col, White, 0.55f)
    val n = 28
    for (i in 0 until n) {
        val f = i / (n - 1f)
        drawCircle(
            lerp(dark, bright, f.pow(1.6f)),
            r * (1f - f * 0.88f),
            c + Offset(-0.30f, -0.36f) * (r * f),
        )
    }

    // 와이어프레임 지구본
    val yaw = phase * 0.9f
    val tilt = 0.45f
    val wireA = 0.20f + 0.55f * k
    val wire = lerp(col, White, 0.6f)
    fun line(pts: List<FloatArray>) {
        for (i in 0 until pts.size - 1) {
            val a = rot3(pts[i], 0f, yaw, tilt)
            val b = rot3(pts[i + 1], 0f, yaw, tilt)
            val z = (a[2] + b[2]) * 0.5f
            if (z <= 0f) continue
            drawLine(wire.a(wireA * z), at(a, r), at(b, r), strokeWidth = 0.8f * d)
        }
    }
    val tau = 2 * PI.toFloat()
    for (i in 0 until 6) {
        val phi = i * PI.toFloat() / 6f
        line((0..48).map { j ->
            val th = j / 48f * tau
            floatArrayOf(sin(th) * cos(phi), cos(th), sin(th) * sin(phi))
        })
    }
    for (lat in listOf(-PI.toFloat() / 4f, 0f, PI.toFloat() / 4f)) {
        line((0..48).map { j ->
            val th = j / 48f * tau
            floatArrayOf(cos(lat) * cos(th), sin(lat), cos(lat) * sin(th))
        })
    }

    // 코어 + 하이라이트 + 림
    drawCircle(White.a(0.10f + 0.30f * k), (7f + pulse * 2.5f * k) * d, c)
    drawCircle(White.a(0.15f + 0.55f * k), 3f * d, c)
    drawCircle(White.a(0.45f), 4f * d, c + Offset(-8f * d, -10f * d))
    drawCircle(col.a(0.18f + 0.15f * k), r, c, style = Stroke(3f * d))
    drawCircle(bright.a(0.45f + 0.4f * pulse * k), r - 0.5f * d, c, style = Stroke(1.2f * d))

    drawRing(true)

    // 링을 따라 도는 광점
    rings.forEachIndexed { i, (roll, tl, dir) ->
        val a = phase * 1.7f * dir + i * 2.1f
        val v = rot3(floatArrayOf(cos(a), 0f, sin(a)), roll, phase * 0.6f * dir, tl)
        if (v[2] < -0.2f && k < 0.99f) return@forEachIndexed
        val pos = at(v, ringR)
        val s = 0.25f + 0.75f * k
        drawCircle(col.a(0.25f * s), 5f * d * s, pos)
        drawCircle(White.a(0.9f * max(s, 0.3f)), 2.2f * d * max(s, 0.5f), pos)
    }
}
