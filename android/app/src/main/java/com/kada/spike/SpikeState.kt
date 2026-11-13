package com.kada.spike

import android.content.Context
import android.content.SharedPreferences
import android.os.SystemClock
import android.util.Log
import java.util.ArrayDeque

/**
 * S0 探针的进程内状态：实验开关（本机持久）、按键统计、事件环、延迟样本、动作结果。
 *
 * 只活在探针里：没有配置同步、没有 Tauri、没有任何跨进程的东西。
 * 线程约定：无障碍回调在主线程，动作也在主线程执行；跨线程读的是 `@Volatile`
 * 与 `synchronized(lock)` 保护的计数。
 */
object SpikeState {
    const val TAG = "KadaS0"

    private const val MAX_EVENTS = 300
    private const val MAX_LATENCY = 500

    private val lock = Any()
    private lateinit var prefs: SharedPreferences

    @Volatile
    var appContext: Context? = null
        private set

    fun init(ctx: Context) {
        if (appContext != null) return
        val app = ctx.applicationContext
        appContext = app
        prefs = app.getSharedPreferences("kada-s0", Context.MODE_PRIVATE)
    }

    private fun persist(block: (SharedPreferences.Editor) -> Unit) {
        val editor = prefs.edit()
        block(editor)
        editor.apply()
    }

    // ---------------------------------------------------------------------
    // 实验开关
    // ---------------------------------------------------------------------

    /** 总闸：关掉 = 所有按键原样放行（对照实验用）。 */
    var swallowMaster: Boolean
        get() = prefs.getBoolean("swallowMaster", true)
        set(value) = persist { it.putBoolean("swallowMaster", value) }

    /** 演示规则：F8 → 气泡、F7 → 复制、F9 短按/长按、Ctrl+Alt+K。 */
    var demoRules: Boolean
        get() = prefs.getBoolean("demoRules", true)
        set(value) = persist { it.putBoolean("demoRules", value) }

    /** 端到端吞键验证：吞掉 A 键（输入框里应按不出 a）。 */
    var swallowA: Boolean
        get() = prefs.getBoolean("swallowA", false)
        set(value) = persist { it.putBoolean("swallowA", value) }

    /** 音量键能不能真的被吞掉：连音量都不该变（手机档的物理触发源全靠这条）。 */
    var swallowVolume: Boolean
        get() = prefs.getBoolean("swallowVolume", false)
        set(value) = persist { it.putBoolean("swallowVolume", value) }

    /** 手机档的物理触发源实验：音量上 + 音量下。 */
    var volumeChord: Boolean
        get() = prefs.getBoolean("volumeChord", false)
        set(value) = persist { it.putBoolean("volumeChord", value) }

    /** 和弦判定模式：false = 非破坏式（只吞凑齐那一下），true = 全吞式。 */
    var chordSwallowAll: Boolean
        get() = prefs.getBoolean("chordSwallowAll", false)
        set(value) = persist { it.putBoolean("chordSwallowAll", value) }

    var tapHoldMs: Int
        get() = prefs.getInt("tapHoldMs", 200).coerceIn(100, 500)
        set(value) = persist { it.putInt("tapHoldMs", value.coerceIn(100, 500)) }

    /** S1：按键判定交给 Rust（配置驱动）；关掉退回 S0 的硬编码演示规则。 */
    var useRustEngine: Boolean
        get() = prefs.getBoolean("useRustEngine", true)
        set(value) = persist { it.putBoolean("useRustEngine", value) }

    // ---------------------------------------------------------------------
    // 服务状态
    // ---------------------------------------------------------------------

    @Volatile
    var a11yConnected = false

    @Volatile
    var filterKeyEventsFlagged = false

    @Volatile
    var foregroundPackage: String? = null

    @Volatile
    var foregroundClass: String? = null

    @Volatile
    var heartbeatStartedElapsed = 0L

    @Volatile
    var heartbeatStartedWall = 0L

    @Volatile
    var heartbeatTicks = 0

    @Volatile
    var lastReportPath: String? = null

    /** UI 上那个输入框的内容：用来做「吞键端到端」的物证（adb input text 打进去的字）。 */
    @Volatile
    var fieldText: String = ""

    /**
     * 输入框是否真的拿到了输入焦点、Activity 窗口是否有焦点。
     *
     * 自动化脚本靠这两条判断「打字前焦点到底在不在」——不然打字全丢只会得到一个空字符串，
     * 分不清是「吞键生效」还是「压根没聚焦」。
     */
    @Volatile
    var fieldFocused: Boolean = false

    @Volatile
    var windowFocused: Boolean = false

    /** 最近一次 `op focusField` 的结果，报告里直接可见。 */
    @Volatile
    var lastFocusAttempt: String = ""

    fun onAccessibilityConnected(connected: Boolean) {
        if (a11yConnected != connected) {
            a11yConnected = connected
            Log.i(TAG, if (connected) "a11y connected" else "a11y disconnected")
        }
        if (!connected) filterKeyEventsFlagged = false
    }

    fun onForeground(pkg: String?, cls: String?) {
        foregroundPackage = pkg
        foregroundClass = cls
    }

    fun onHeartbeatStarted() {
        heartbeatStartedElapsed = SystemClock.elapsedRealtime()
        heartbeatStartedWall = System.currentTimeMillis()
        heartbeatTicks = 0
        Log.i(TAG, "heartbeat started")
    }

    fun onHeartbeatStopped() {
        val minutes = if (heartbeatStartedElapsed == 0L) 0.0
        else (SystemClock.elapsedRealtime() - heartbeatStartedElapsed) / 60_000.0
        heartbeatStartedElapsed = 0L
        Log.i(TAG, "heartbeat stopped after %.1f min, ticks=%d".format(minutes, heartbeatTicks))
    }

    fun heartbeatTick() {
        heartbeatTicks++
    }

    fun heartbeatRunning(): Boolean = heartbeatStartedElapsed > 0L

    // ---------------------------------------------------------------------
    // 按键统计
    // ---------------------------------------------------------------------

    data class KeyLog(
        val wallMs: Long,
        val uptimeMs: Long,
        val kind: String,
        val key: String,
        val keyCode: Int,
        val repeat: Int,
        val modifiers: String,
        val deviceId: Int,
        val pipelineMs: Long,
        val swallowed: Boolean,
        val note: String,
    )

    data class StatsSnapshot(
        val down: Int,
        val up: Int,
        val repeat: Int,
        val unmatchedUp: Int,
        val swallowed: Int,
        val passed: Int,
        val actionsFired: Int,
        val actionsFailed: Int,
        val heldKeys: List<String>,
    )

    data class LatencySummary(
        val count: Int,
        val p50: Double,
        val p95: Double,
        val max: Double,
        val avg: Double,
    )

    data class ActionRecord(
        val id: String,
        val lastOk: Boolean?,
        val lastDetail: String,
        val lastReason: String,
        val lastAtMs: Long,
        val okCount: Int,
        val failCount: Int,
    )

    private var downCount = 0
    private var upCount = 0
    private var repeatCount = 0
    private var unmatchedUpCount = 0
    private var swallowedCount = 0
    private var passedCount = 0
    private var actionsFired = 0
    private var actionsFailed = 0

    private val heldCodes = LinkedHashSet<Int>()
    private val recent = ArrayDeque<KeyLog>(MAX_EVENTS)
    private val latencySamples = ArrayDeque<Double>(MAX_LATENCY)
    private val actions = LinkedHashMap<String, ActionRecord>()

    /** S1：Rust 判定触发过的快捷键（最近 N 条，界面上直接看「按下去有没有执行」）。 */
    data class RustFire(
        val atMs: Long,
        val trigger: String,
        val name: String,
        val ok: Boolean,
        val detail: String,
    )

    private val rustFires = ArrayDeque<RustFire>(50)

    @Volatile
    var rustDecisions = 0

    fun noteRustDecision() {
        rustDecisions++
    }

    fun noteRustFire(trigger: String, name: String, ok: Boolean, detail: String) {
        synchronized(lock) {
            if (rustFires.size >= 50) rustFires.removeFirst()
            rustFires.addLast(RustFire(System.currentTimeMillis(), trigger, name, ok, detail))
        }
    }

    fun rustFireLog(): List<RustFire> = synchronized(lock) { rustFires.toList() }

    fun noteKey(raw: RawKey, swallow: Boolean, note: String) {
        val entry = KeyLog(
            wallMs = System.currentTimeMillis(),
            uptimeMs = raw.recvUptimeMs,
            kind = raw.kind,
            key = raw.keyName,
            keyCode = raw.keyCode,
            repeat = raw.repeat,
            modifiers = raw.modifierLabel,
            deviceId = raw.deviceId,
            pipelineMs = raw.pipelineMs,
            swallowed = swallow,
            note = note,
        )
        synchronized(lock) {
            when (raw.kind) {
                "repeat" -> repeatCount++
                "up" -> {
                    upCount++
                    if (!heldCodes.remove(raw.keyCode)) unmatchedUpCount++
                }
                else -> {
                    downCount++
                    heldCodes.add(raw.keyCode)
                }
            }
            if (swallow) swallowedCount++ else passedCount++
            if (recent.size >= MAX_EVENTS) recent.removeFirst()
            recent.addLast(entry)
        }
        Log.i(
            TAG,
            "key ${entry.kind} ${entry.key} code=${entry.keyCode} repeat=${entry.repeat} " +
                "mods=${entry.modifiers} dev=${entry.deviceId} swallow=$swallow " +
                "pipe=${entry.pipelineMs}ms note=$note",
        )
    }

    fun addLatency(ms: Double) {
        synchronized(lock) {
            if (latencySamples.size >= MAX_LATENCY) latencySamples.removeFirst()
            latencySamples.addLast(ms)
        }
    }

    fun recordAction(id: String, ok: Boolean, detail: String, reason: String) {
        synchronized(lock) {
            val prev = actions[id]
            val okCount = (prev?.okCount ?: 0) + if (ok) 1 else 0
            val failCount = (prev?.failCount ?: 0) + if (ok) 0 else 1
            if (ok) actionsFired++ else actionsFailed++
            actions[id] = ActionRecord(
                id = id,
                lastOk = ok,
                lastDetail = detail,
                lastReason = reason,
                lastAtMs = System.currentTimeMillis(),
                okCount = okCount,
                failCount = failCount,
            )
        }
        Log.i(TAG, "action id=$id ok=$ok detail=$detail reason=$reason")
    }

    fun stats(): StatsSnapshot = synchronized(lock) {
        StatsSnapshot(
            down = downCount,
            up = upCount,
            repeat = repeatCount,
            unmatchedUp = unmatchedUpCount,
            swallowed = swallowedCount,
            passed = passedCount,
            actionsFired = actionsFired,
            actionsFailed = actionsFailed,
            heldKeys = heldCodes.map { KeyNames.name(it) },
        )
    }

    fun events(): List<KeyLog> = synchronized(lock) { recent.toList() }

    fun actionRecords(): Map<String, ActionRecord> = synchronized(lock) { LinkedHashMap(actions) }

    fun latencySummary(): LatencySummary = synchronized(lock) {
        if (latencySamples.isEmpty()) {
            LatencySummary(0, 0.0, 0.0, 0.0, 0.0)
        } else {
            val sorted = latencySamples.sorted()
            LatencySummary(
                count = sorted.size,
                p50 = percentile(sorted, 50),
                p95 = percentile(sorted, 95),
                max = sorted.last(),
                avg = sorted.average(),
            )
        }
    }

    private fun percentile(sorted: List<Double>, p: Int): Double {
        val index = ((sorted.size * p + 99) / 100 - 1).coerceIn(0, sorted.size - 1)
        return sorted[index]
    }

    fun clear() {
        synchronized(lock) {
            downCount = 0
            upCount = 0
            repeatCount = 0
            unmatchedUpCount = 0
            swallowedCount = 0
            passedCount = 0
            actionsFired = 0
            actionsFailed = 0
            heldCodes.clear()
            recent.clear()
            latencySamples.clear()
            actions.clear()
            rustFires.clear()
            rustDecisions = 0
        }
        Log.i(TAG, "stats cleared")
    }

    /** 一行摘要写进 logcat，自动化脚本只 grep 这一行就能判定。 */
    fun logSnapshot() {
        val s = stats()
        val l = latencySummary()
        Log.i(
            TAG,
            "snapshot keys=${s.down} up=${s.up} repeat=${s.repeat} unmatchedUp=${s.unmatchedUp} " +
                "swallowed=${s.swallowed} passed=${s.passed} actions=${s.actionsFired} " +
                "failed=${s.actionsFailed} latencyN=${l.count} p50=%.2f p95=%.2f".format(l.p50, l.p95),
        )
    }
}
