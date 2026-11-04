package com.adblocker

import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.content.ServiceConnection
import android.net.ConnectivityManager
import android.net.NetworkCapabilities
import android.os.Handler
import android.os.IBinder
import android.os.Looper
import android.os.SystemClock
import android.util.Base64
import de.blinkt.openvpn.core.ConfigParser
import de.blinkt.openvpn.core.ConnectionStatus
import de.blinkt.openvpn.core.IOpenVPNServiceInternal
import de.blinkt.openvpn.core.OpenVPNService
import de.blinkt.openvpn.core.ProfileManager
import de.blinkt.openvpn.core.VPNLaunchHelper
import de.blinkt.openvpn.core.VpnStatus
import java.io.File
import java.io.StringReader
import java.net.DatagramPacket
import java.net.DatagramSocket
import java.net.HttpURLConnection
import java.net.InetAddress
import java.net.InetSocketAddress
import java.net.Socket
import java.net.URL
import java.util.Locale
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean

class VpnServer(val ip: String, val speed: Long, val ping: Int, val configB64: String)

class VpnCountry(val short: String, val name: String, val servers: List<VpnServer>) {
    val label: String get() = KO_NAMES[short] ?: name
    val bestSpeed: Long get() = servers.maxOfOrNull { it.speed } ?: 0L
    val bestPing: Int? get() = servers.filter { it.speed > 0 }.minByOrNull { it.ping }?.ping

    companion object {
        val KO_NAMES = mapOf(
            "JP" to "일본", "KR" to "대한민국", "US" to "미국", "RU" to "러시아", "TH" to "태국",
            "CA" to "캐나다", "HR" to "크로아티아", "RO" to "루마니아", "PT" to "포르투갈", "GB" to "영국",
            "AT" to "오스트리아", "TW" to "대만", "LV" to "라트비아", "DE" to "독일", "FR" to "프랑스",
            "NL" to "네덜란드", "SG" to "싱가포르", "VN" to "베트남", "HK" to "홍콩", "IN" to "인도",
            "ID" to "인도네시아", "UA" to "우크라이나", "PL" to "폴란드", "CH" to "스위스", "SE" to "스웨덴",
            "ES" to "스페인", "IT" to "이탈리아", "BR" to "브라질", "AU" to "호주", "CN" to "중국",
            "MY" to "말레이시아", "PH" to "필리핀", "TR" to "튀르키예",
        )
    }
}

class VpnList(val countries: List<VpnCountry>, val ageDays: Long, val stale: Boolean)

/** 접속 진행 상황 (모두 메인 스레드에서 전달된다). */
sealed interface VpnEvent {
    class Probe(val alive: Int, val total: Int, val secs: Float) : VpnEvent
    class Attempt(val n: Int, val ip: String, val ok: Boolean, val reason: String, val secs: Float) : VpnEvent
    /** 후보가 모두 실패해서 목록을 일찍 새로 받았다. */
    data object ListRefreshed : VpnEvent
}

/**
 * VPN Gate(쓰쿠바 대학) 공개 서버 목록.
 *
 * - 목록은 전부 받아 캐시하고, 마지막 요청으로부터 3일이 지나야 다시 받는다.
 * - 터널은 앱에 내장된 OpenVPN(ics-openvpn 코어)이 직접 연다.
 */
object VpnController {
    private const val API_URL = "https://www.vpngate.net/api/iphone/"
    private const val MAX_AGE_MS = 3L * 24 * 60 * 60 * 1000
    private const val CACHE_NAME = "vpngate-cache.csv"
    private const val CONNECT_TIMEOUT_MS = 12_000L
    private const val MAX_ATTEMPTS = 6
    private const val PROBE_TCP_TIMEOUT_MS = 2_500
    private const val PROBE_UDP_WAIT_MS = 1_200
    private const val PROBE_TOTAL_SEC = 4L
    /** 블랙리스트 기간: 무응답/시간 초과 6시간, 설정 거부 24시간, 인증 거부 1시간. */
    private const val TTL_DEAD = 6L * 3600 * 1000
    private const val TTL_REJECT = 24L * 3600 * 1000
    /** 인증 거부는 동시 접속 한도 초과 등 일시적인 경우가 많아 짧게 제외한다. */
    private const val TTL_AUTH = 3600L * 1000
    /** 모든 후보가 실패했는데 캐시가 이보다 오래됐다면 3일 규칙의 예외로 목록을 한 번 새로 받는다. */
    private const val REFRESH_AFTER_MS = 12L * 3600 * 1000

    // ---------- 상태 ----------

    /** 어떤 VPN 이든 지금 활성화되어 있는가 (시스템 값이라 다른 앱으로 접속해도 감지됨). */
    fun isActive(ctx: Context): Boolean {
        val cm = ctx.getSystemService(Context.CONNECTIVITY_SERVICE) as ConnectivityManager
        val caps = cm.getNetworkCapabilities(cm.activeNetwork) ?: return false
        return caps.hasTransport(NetworkCapabilities.TRANSPORT_VPN)
    }

    // ---------- 목록 ----------

    /** 블로킹 — 백그라운드 스레드에서 호출. 캐시가 3일 이내면 네트워크 요청 없음. */
    fun loadList(ctx: Context): Result<VpnList> = runCatching {
        val file = File(ctx.filesDir, CACHE_NAME)
        val age = if (file.exists()) System.currentTimeMillis() - file.lastModified() else Long.MAX_VALUE
        var stale = false
        if (age > MAX_AGE_MS) {
            try {
                download(file)
            } catch (e: Exception) {
                if (!file.exists()) throw e
                stale = true
            }
        }
        val countries = parse(file.readText())
        if (countries.isEmpty()) {
            file.delete()
            error("사용 가능한 서버가 없습니다")
        }
        val days = (System.currentTimeMillis() - file.lastModified()) / 86_400_000L
        VpnList(countries, days, stale)
    }

    private fun download(dest: File) {
        val conn = URL(API_URL).openConnection() as HttpURLConnection
        conn.connectTimeout = 15_000
        conn.readTimeout = 60_000
        try {
            if (conn.responseCode != 200) error("목록을 받지 못했습니다 (HTTP ${conn.responseCode})")
            val text = conn.inputStream.bufferedReader().use { it.readText() }
            if (!text.startsWith("*vpn_servers")) error("서버 목록 형식이 올바르지 않습니다")
            val tmp = File(dest.parentFile, "$CACHE_NAME.part")
            tmp.writeText(text)
            if (!tmp.renameTo(dest)) {
                dest.delete()
                tmp.renameTo(dest)
            }
        } finally {
            conn.disconnect()
        }
    }

    internal fun parse(csv: String): List<VpnCountry> {
        val map = LinkedHashMap<String, Pair<String, MutableList<VpnServer>>>()
        for (line in csv.lineSequence()) {
            if (line.isBlank() || line.startsWith("*") || line.startsWith("#")) continue
            val f = line.trimEnd().split(',')
            if (f.size < 15) continue
            val short = f[6].trim().uppercase(Locale.ROOT)
            val config = f.last().trim()
            if (short.length != 2 || config.isEmpty()) continue
            val server = VpnServer(
                ip = f[1].trim(),
                speed = f[4].trim().toLongOrNull() ?: 0L,
                ping = f[3].trim().toIntOrNull() ?: 0,
                configB64 = config,
            )
            map.getOrPut(short) { f[5].trim() to mutableListOf() }.second.add(server)
        }
        // 일본 최상단, 나머지는 서버가 많은 순 → 이름순
        return map.map { (s, v) -> VpnCountry(s, v.first, v.second) }
            .sortedWith(
                compareByDescending<VpnCountry> { it.short == "JP" }
                    .thenByDescending { it.servers.size }
                    .thenBy { it.name }
            )
    }

    // ---------- config 검증 ----------

    private val ALLOWED = setOf(
        "client", "dev", "dev-type", "proto", "remote", "cipher", "data-ciphers", "data-ciphers-fallback", "auth",
        "resolv-retry", "nobind", "persist-key", "persist-tun", "verb", "remote-cert-tls", "tls-client", "pull",
        "key-direction", "tun-mtu", "mssfix", "reneg-sec", "keepalive", "ping", "ping-restart", "connect-retry",
        "connect-timeout", "tls-version-min", "tls-cipher", "tls-ciphersuites", "ca", "cert", "key", "tls-auth",
        "tls-crypt", "auth-nocache", "float", "explicit-exit-notify",
    )
    private val BLOCKS = setOf("ca", "cert", "key", "tls-auth", "tls-crypt")
    private val TOKEN = Regex("^[A-Za-z0-9\\-._:+/=]+$")
    private val PEM = Regex("^[A-Za-z0-9+/=]*$")
    private val PEM_HEADER = Regex("^-----[A-Za-z0-9 ]+-----$")
    private val HEXCOLON = Regex("^[0-9A-Fa-f:]+$")

    /** 허용 목록에 없는 지시어가 있으면 config 전체를 거부한다 (자원봉사자 서버 입력은 신뢰하지 않음). */
    internal fun sanitize(text: String): String {
        val out = StringBuilder()
        var block: String? = null
        for (raw in text.lines()) {
            val line = raw.trim()
            val open = block
            if (open != null) {
                if (line == "</$open>") block = null
                else require(line.isEmpty() || PEM.matches(line) || PEM_HEADER.matches(line) || HEXCOLON.matches(line)) {
                    "config 인라인 블록에 허용되지 않는 내용"
                }
                out.append(line).append('\n')
                continue
            }
            if (line.isEmpty() || line.startsWith("#") || line.startsWith(";")) continue
            if (line.startsWith("<") && line.endsWith(">")) {
                val tag = line.substring(1, line.length - 1)
                require(tag in BLOCKS) { "허용되지 않는 블록 <$tag>" }
                block = tag
                out.append(line).append('\n')
                continue
            }
            val parts = line.split(Regex("\\s+"))
            val key = parts[0]
            val args = parts.drop(1)
            require(key in ALLOWED) { "허용되지 않는 지시어 '$key' — 접속하지 않습니다" }
            require(key !in BLOCKS || args.isEmpty()) { "'$key' 외부 파일 참조는 허용되지 않습니다" }
            require(args.all { TOKEN.matches(it) }) { "'$key' 인자에 허용되지 않는 문자" }
            if (key == "proto") require(args.all { it in setOf("udp", "tcp", "udp4", "tcp4", "udp6", "tcp6", "tcp-client") }) { "지원하지 않는 proto" }
            if (key == "dev") require(args.all { it == "tun" || it == "tap" }) { "지원하지 않는 dev" }
            out.append(line).append('\n')
        }
        require(block == null) { "config 블록이 닫히지 않았습니다" }
        require(out.startsWith("remote ") || out.contains("\nremote ")) { "remote 지시어가 없습니다" }
        return out.toString()
    }

    // ---------- 사전 탐색 (죽은 서버를 OpenVPN 을 띄우기 전에 걸러낸다) ----------

    private class Endpoint(val host: String, val port: Int, val udp: Boolean)

    private fun endpoint(s: VpnServer): Endpoint? = runCatching {
        var udp = true
        var remote: Pair<String, Int>? = null
        for (line in String(Base64.decode(s.configB64, Base64.DEFAULT), Charsets.UTF_8).lineSequence()) {
            val t = line.trim().split(Regex("\\s+"))
            when {
                t[0] == "proto" -> udp = t.getOrNull(1)?.startsWith("udp") ?: true
                t[0] == "remote" && remote == null && t.size >= 3 -> remote = t[1] to t[2].toInt()
            }
        }
        remote?.let { Endpoint(it.first, it.second, udp) }
    }.getOrNull()

    private fun udpProbe(addr: InetSocketAddress): Boolean = runCatching {
        DatagramSocket().use { sock ->
            sock.soTimeout = PROBE_UDP_WAIT_MS
            sock.connect(addr)
            // OpenVPN P_CONTROL_HARD_RESET_CLIENT_V2: opcode(7<<3) + session id(8) + ack 개수(0) + packet id(4)
            val pkt = ByteArray(14)
            pkt[0] = (7 shl 3).toByte()
            val sid = ByteArray(8).also { java.util.Random().nextBytes(it) }
            System.arraycopy(sid, 0, pkt, 1, 8)
            repeat(2) {
                sock.send(DatagramPacket(pkt, pkt.size))
                val buf = ByteArray(64)
                val resp = DatagramPacket(buf, buf.size)
                try {
                    sock.receive(resp)
                    // 서버는 P_CONTROL_HARD_RESET_SERVER_V2(8<<3) 로 답한다. (tls-auth/crypt 를 쓰지 않는 VPN Gate 설정 기준)
                    if (resp.length > 0 && (buf[0].toInt() and 0xFF) shr 3 == 8) return@runCatching true
                } catch (_: java.net.SocketTimeoutException) {
                }
            }
            false
        }
    }.getOrDefault(false)

    private fun probe(e: Endpoint): Boolean {
        val addr = runCatching { InetSocketAddress(InetAddress.getByName(e.host), e.port) }.getOrNull()
            ?: return true // 해석 못 하면 판단 보류(제외하지 않음)
        return if (e.udp) udpProbe(addr)
        else runCatching { Socket().use { it.connect(addr, PROBE_TCP_TIMEOUT_MS); true } }.getOrDefault(false)
    }

    /** 후보 전체를 병렬로 탐색. 제한 시간 안에 답이 없으면 죽은 것으로 본다. */
    private fun probeAll(servers: List<VpnServer>): List<Boolean> {
        val res = List(servers.size) { AtomicBoolean(false) }
        val latch = CountDownLatch(servers.size)
        servers.forEachIndexed { i, s ->
            Thread {
                try {
                    res[i].set(endpoint(s)?.let { probe(it) } ?: true)
                } finally {
                    latch.countDown()
                }
            }.start()
        }
        latch.await(PROBE_TOTAL_SEC, TimeUnit.SECONDS)
        return res.map { it.get() }
    }

    // ---------- 블랙리스트 (재시작해도 유지) ----------

    private fun blFile(ctx: Context) = File(ctx.filesDir, "vpn-blacklist.txt")

    /** 만료되지 않은 항목만. ip -> 만료 시각(ms) */
    @Synchronized
    private fun blLoad(ctx: Context): MutableMap<String, Long> {
        val f = blFile(ctx)
        if (!f.exists()) return mutableMapOf()
        val now = System.currentTimeMillis()
        return f.readLines().mapNotNull { l ->
            val p = l.split('\t')
            val exp = p.getOrNull(1)?.trim()?.toLongOrNull()
            if (p.size >= 2 && exp != null && exp > now) p[0] to exp else null
        }.toMap().toMutableMap()
    }

    @Synchronized
    private fun blUpdate(ctx: Context, f: (MutableMap<String, Long>) -> Unit) {
        val m = blLoad(ctx)
        f(m)
        blFile(ctx).writeText(m.entries.joinToString("") { "${it.key}\t${it.value}\n" })
    }

    private fun blAdd(ctx: Context, ip: String, ttlMs: Long) =
        blUpdate(ctx) { it[ip] = System.currentTimeMillis() + ttlMs }

    private fun blRemove(ctx: Context, ip: String) = blUpdate(ctx) { it.remove(ip) }

    // ---------- 접속 (내장 OpenVPN) ----------

    private val main = Handler(Looper.getMainLooper())
    private var session: Session? = null

    private class Session(
        val short: String,
        val onEvent: (VpnEvent) -> Unit,
        val onConnected: (ip: String, totalSecs: Float) -> Unit,
        val onFailed: (String) -> Unit,
    ) {
        val t0 = SystemClock.elapsedRealtime()
        var n = 0
        var refreshed = false
        var candidates: List<VpnServer> = emptyList()
        var index = 0
        var lastError = "응답하는 서버가 없습니다"
        var attemptStart = 0L
        var listener: VpnStatus.StateListener? = null
        var timeout: Runnable? = null
        var done = false
    }

    private fun secsSince(t: Long) = (SystemClock.elapsedRealtime() - t) / 1000f

    /** 속도를 아는 서버는 빠른 순, 모르는 서버는 무작위로 그 뒤에. */
    private fun order(v: List<VpnServer>): List<VpnServer> =
        v.filter { it.speed > 0 }.sortedByDescending { it.speed } + v.filter { it.speed == 0L }.shuffled()

    private fun readCountry(ctx: Context, short: String): VpnCountry? = runCatching {
        parse(File(ctx.filesDir, CACHE_NAME).readText()).firstOrNull { it.short == short }
    }.getOrNull()

    /**
     * 사용자가 경고 모달에서 "접속"을 눌렀고, VpnService 권한까지 받은 뒤에만 호출한다.
     * 국가의 서버를 먼저 병렬 탐색해 죽은 서버를 거르고, 빠른 순으로 최대 6개를 시도한다.
     * 콜백은 모두 메인 스레드에서 불린다.
     */
    fun connect(
        ctx: Context,
        short: String,
        onEvent: (VpnEvent) -> Unit,
        onConnected: (ip: String, totalSecs: Float) -> Unit,
        onFailed: (String) -> Unit,
    ) {
        val app = ctx.applicationContext
        cancelSession(app, stopTunnel = true)
        if (isActive(app)) stopTunnel(app) // 전환: 탐색 패킷이 기존 터널을 타지 않도록 먼저 내린다
        val s = Session(short, onEvent, onConnected, onFailed)
        session = s
        startRound(app, s)
    }

    private fun startRound(ctx: Context, s: Session) {
        Thread {
            val w0 = SystemClock.elapsedRealtime()
            while (isActive(ctx) && SystemClock.elapsedRealtime() - w0 < 3000) Thread.sleep(100)

            val country = readCountry(ctx, s.short)
            if (country == null) {
                main.post { if (!s.done) finish(ctx, s) { s.onFailed("서버 목록을 읽을 수 없습니다") } }
                return@Thread
            }
            val bl = blLoad(ctx)
            var cands = country.servers.filter { it.ip !in bl }
            if (cands.isEmpty()) cands = country.servers // 전부 제외돼 있으면 이번만 블랙리스트를 무시

            val t = SystemClock.elapsedRealtime()
            val flags = probeAll(cands)
            val alive = cands.filterIndexed { i, _ -> flags[i] }
            cands.filterIndexed { i, _ -> !flags[i] }.forEach { blAdd(ctx, it.ip, TTL_DEAD) }
            val secs = secsSince(t)

            main.post {
                if (s.done) return@post
                s.onEvent(VpnEvent.Probe(alive.size, cands.size, secs))
                s.candidates = order(alive).take(MAX_ATTEMPTS)
                s.index = 0
                tryNext(ctx, s)
            }
        }.start()
    }

    private fun tryNext(ctx: Context, s: Session) {
        if (s.done) return
        detach(s)
        if (s.index >= s.candidates.size) {
            exhausted(ctx, s)
            return
        }
        val server = s.candidates[s.index++]
        s.n++
        s.attemptStart = SystemClock.elapsedRealtime()
        try {
            // VPN Gate 서버는 설정 파일에 계정이 없고, 사용자명/비밀번호 "vpn"/"vpn" 을 요구한다.
            // (허용 목록 검증을 통과한 뒤에 우리가 직접 덧붙인다 — 외부 입력은 아님)
            val config = sanitize(String(Base64.decode(server.configB64, Base64.DEFAULT), Charsets.UTF_8)) +
                "auth-user-pass\n"
            val parser = ConfigParser()
            parser.parseConfig(StringReader(config))
            val profile = parser.convertProfile()
            profile.mUsername = "vpn"
            profile.mPassword = "vpn"
            profile.mName = "VPNGate ${server.ip}"

            val listener = object : VpnStatus.StateListener {
                // 리스너를 등록하면 코어가 "마지막 상태"를 바로 재생한다. 직전 터널이 연결됨이었다면
                // 새 접속이 시작되기도 전에 CONNECTED 가 와서 가짜 성공이 되므로,
                // 새 시도에서 "연결 중" 계열 상태를 한 번 본 뒤의 CONNECTED 만 인정한다.
                var progressed = false

                override fun updateState(state: String?, msg: String?, resId: Int, level: ConnectionStatus?, i: Intent?) {
                    main.post {
                        if (s.done || s.listener !== this) return@post
                        if (level != null && level != ConnectionStatus.LEVEL_CONNECTED) progressed = true
                        when (level) {
                            ConnectionStatus.LEVEL_CONNECTED -> if (progressed) succeed(ctx, s, server)
                            ConnectionStatus.LEVEL_AUTH_FAILED -> attemptFailed(ctx, s, server, "인증 거부", TTL_AUTH)
                            else -> {}
                        }
                    }
                }

                override fun setConnectedVPN(uuid: String?) {}
            }
            s.listener = listener
            VpnStatus.addStateListener(listener)
            s.timeout = Runnable { attemptFailed(ctx, s, server, "시간 초과", TTL_DEAD) }
                .also { main.postDelayed(it, CONNECT_TIMEOUT_MS) }

            ProfileManager.setTemporaryProfile(ctx, profile)
            VPNLaunchHelper.startOpenVpn(profile, ctx, "adblocker", true)
        } catch (e: Exception) {
            attemptFailed(ctx, s, server, "설정 거부", TTL_REJECT, stop = false)
        }
    }

    private fun succeed(ctx: Context, s: Session, server: VpnServer) {
        blRemove(ctx, server.ip)
        s.onEvent(VpnEvent.Attempt(s.n, server.ip, true, "연결", secsSince(s.attemptStart)))
        val total = secsSince(s.t0)
        finish(ctx, s, stop = false) { s.onConnected(server.ip, total); refreshWidgets(ctx) }
    }

    private fun attemptFailed(ctx: Context, s: Session, server: VpnServer, reason: String, ttlMs: Long, stop: Boolean = true) {
        if (s.done) return
        if (stop) stopTunnel(ctx)
        detach(s)
        blAdd(ctx, server.ip, ttlMs)
        s.lastError = reason
        s.onEvent(VpnEvent.Attempt(s.n, server.ip, false, reason, secsSince(s.attemptStart)))
        tryNext(ctx, s)
    }

    /** 후보가 모두 실패. 캐시가 오래됐다면 목록을 일찍 새로 받아 한 번 더 시도한다. */
    private fun exhausted(ctx: Context, s: Session) {
        val file = File(ctx.filesDir, CACHE_NAME)
        val old = !file.exists() || System.currentTimeMillis() - file.lastModified() >= REFRESH_AFTER_MS
        if (!s.refreshed && old) {
            s.refreshed = true
            Thread {
                val ok = runCatching { download(file) }.isSuccess
                main.post {
                    if (s.done) return@post
                    if (ok) {
                        s.onEvent(VpnEvent.ListRefreshed)
                        startRound(ctx, s)
                    } else finish(ctx, s) { s.onFailed("접속하지 못했습니다 (${s.lastError})") }
                }
            }.start()
        } else {
            finish(ctx, s) { s.onFailed("접속하지 못했습니다 (${s.lastError})") }
        }
    }

    private fun detach(s: Session) {
        s.timeout?.let { main.removeCallbacks(it) }
        s.timeout = null
        s.listener?.let { VpnStatus.removeStateListener(it) }
        s.listener = null
    }

    private fun finish(ctx: Context, s: Session, stop: Boolean = true, cb: () -> Unit) {
        s.done = true
        detach(s)
        if (session === s) session = null
        if (stop) stopTunnel(ctx)
        cb()
    }

    private fun cancelSession(ctx: Context, stopTunnel: Boolean) {
        val s = session ?: return
        s.done = true
        detach(s)
        session = null
        if (stopTunnel) stopTunnel(ctx)
    }

    /** 접속 시도 취소 + 연결 끊기. */
    fun disconnect(ctx: Context) {
        cancelSession(ctx.applicationContext, stopTunnel = false)
        stopTunnel(ctx.applicationContext)
        refreshWidgets(ctx)
    }

    /** VPN 상태는 시스템이 잠시 뒤에 반영하므로 약간 기다렸다가 위젯을 다시 그린다. */
    private fun refreshWidgets(ctx: Context) {
        val app = ctx.applicationContext
        main.postDelayed({ DnsWidget.updateAll(app) }, 1500)
    }

    /** OpenVPNService 에 바인드해서 터널을 내린다. */
    private fun stopTunnel(ctx: Context) {
        val intent = Intent(ctx, OpenVPNService::class.java).setAction(OpenVPNService.START_SERVICE)
        val conn = object : ServiceConnection {
            override fun onServiceConnected(name: ComponentName?, binder: IBinder?) {
                runCatching { IOpenVPNServiceInternal.Stub.asInterface(binder).stopVPN(false) }
                runCatching { ctx.unbindService(this) }
            }

            override fun onServiceDisconnected(name: ComponentName?) {}
        }
        // 서비스가 떠 있지 않으면 BIND_AUTO_CREATE 로 새로 만들지 않도록 플래그 없이 바인드
        runCatching { ctx.bindService(intent, conn, 0) }
    }
}
