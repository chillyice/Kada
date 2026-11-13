package com.kada.spike

import android.accessibilityservice.AccessibilityService
import android.accessibilityservice.AccessibilityServiceInfo
import android.content.Intent
import android.os.Handler
import android.os.Looper
import android.util.Log
import android.view.KeyEvent
import android.view.accessibility.AccessibilityEvent

/**
 * S0-1 的探针：声明 `canRequestFilterKeyEvents` + `FLAG_REQUEST_FILTER_KEY_EVENTS`，
 * 在 `onKeyEvent` 里把按键交给 [KeyRouter]，并回答「吞不吞」。
 *
 * 回调里只做判定（微秒级），动作一律 post 到主线程执行——桌面版「回调里不做耗时操作」
 * 的规矩在 Android 上同样成立，只是这次会卡住的是输入系统而不是钩子链。
 */
class KeyFilterService : AccessibilityService() {

    private val main = Handler(Looper.getMainLooper())
    private var router: KeyRouter? = null
    private var actions: SemanticActions? = null
    private var runner: ActionRunner? = null
    private var ticker: Runnable? = null

    override fun onServiceConnected() {
        super.onServiceConnected()
        val info = serviceInfo ?: AccessibilityServiceInfo()
        info.flags = info.flags or AccessibilityServiceInfo.FLAG_REQUEST_FILTER_KEY_EVENTS
        serviceInfo = info

        actions = SemanticActions(this)
        runner = ActionRunner(this)
        router = KeyRouter(HandlerScheduler(main)) { actionId, reason, recvNanos ->
            dispatch(actionId, reason, recvNanos)
        }
        SpikeState.filterKeyEventsFlagged =
            (info.flags and AccessibilityServiceInfo.FLAG_REQUEST_FILTER_KEY_EVENTS) != 0
        SpikeState.onAccessibilityConnected(true)
        instance = this

        // S1：配置驱动判定。初始化失败（.so 缺失）时自动退回 S0 的硬编码演示规则。
        RustBridge.init(this)
        RustBridge.reset()
        startTicker()
        Log.i(SpikeState.TAG, "a11y connected flags=0x${info.flags.toString(16)} rust=${RustBridge.available}")
    }

    /** 序列 / 和弦的等待窗靠定时器落地（等价桌面 `Engine::tick`）。 */
    private fun startTicker() {
        if (ticker != null) return
        val task = object : Runnable {
            override fun run() {
                RustBridge.tick()
                main.postDelayed(this, TICK_MS)
            }
        }
        ticker = task
        main.postDelayed(task, TICK_MS)
    }

    private fun stopTicker() {
        ticker?.let { main.removeCallbacks(it) }
        ticker = null
    }

    override fun onKeyEvent(event: KeyEvent): Boolean {
        val recvNanos = System.nanoTime()
        val raw = RawKey.from(event, recvNanos)

        // S1 主线：判定在 Rust（与桌面同一份 kada-core），这里只搬运 + 执行。
        if (SpikeState.useRustEngine && RustBridge.available) {
            val decision = RustBridge.decide(
                keyName = raw.keyName,
                down = raw.isDown,
                repeat = raw.repeat > 0,
                mods = KeyNames.modifierCsv(event.metaState),
            )
            if (decision != null) {
                SpikeState.noteRustDecision()
                SpikeState.noteKey(raw, decision.swallow, "rust:${decision.note}")
                for (fired in decision.fired) {
                    runner?.run(fired, decision.note)
                }
                return decision.swallow
            }
        }

        val swallow = router?.onKey(raw) ?: false
        return swallow
    }

    override fun onAccessibilityEvent(event: AccessibilityEvent?) {
        if (event == null) return
        when (event.eventType) {
            AccessibilityEvent.TYPE_WINDOW_STATE_CHANGED -> {
                val pkg = event.packageName?.toString()
                SpikeState.onForeground(pkg, event.className?.toString())
                RustBridge.setForeground(pkg)
            }
        }
    }

    override fun onInterrupt() {
        Log.i(SpikeState.TAG, "a11y interrupt")
    }

    override fun onUnbind(intent: Intent?): Boolean {
        Log.i(SpikeState.TAG, "a11y unbind")
        teardown()
        return super.onUnbind(intent)
    }

    override fun onDestroy() {
        teardown()
        super.onDestroy()
    }

    private fun teardown() {
        SpikeState.onAccessibilityConnected(false)
        stopTicker()
        RustBridge.reset()
        router?.shutdown()
        router = null
        actions = null
        runner = null
        instance = null
    }

    private fun dispatch(actionId: String, reason: String, recvNanos: Long) {
        main.post {
            val started = System.nanoTime()
            // 延迟口径：无障碍回调收到 → 动作真正开始执行（含主线程排队）。
            SpikeState.addLatency((started - recvNanos) / 1_000_000.0)
            val result = actions?.perform(actionId, reason) ?: ActionResult(false, "无障碍服务已断开")
            SpikeState.recordAction(actionId, result.ok, result.detail, reason)
        }
    }

    /** 探针用：直接走一次 Rust 判定（不经过物理按键），并把命中的动作真的执行掉。 */
    fun decideForTest(
        key: String,
        down: Boolean,
        repeat: Boolean,
        mods: String,
    ): RustBridge.Decision? {
        val decision = RustBridge.decide(key, down, repeat, mods) ?: return null
        if (decision.note.isNotEmpty()) SpikeState.noteRustDecision()
        for (fired in decision.fired) runner?.run(fired, "test")
        return decision
    }

    /** 探针用：起一次 Rust 判定链前的复位（避免上一条 chord / sequence 的残留）。 */
    fun resetForTest() {
        RustBridge.reset()
    }

    /** 探针用：adb / 界面按钮直接触发一条动作（不经过按键）。 */
    fun dispatchForTest(actionId: String) {
        dispatch(actionId, "test", System.nanoTime())
    }

    /** 探针用：打印聚焦节点的属性与动作列表（诊断 `performAction` 返回 false 的原因）。 */
    fun describeFocusForTest() {
        main.post {
            Log.i(SpikeState.TAG, "focus ${actions?.describeFocus() ?: "无障碍服务已断开"}")
        }
    }

    companion object {
        /** 状态机定时推进间隔（毫秒）：与桌面版 `ENGINE_TICK_MS` 同口径。 */
        private const val TICK_MS = 60L

        @Volatile
        var instance: KeyFilterService? = null
            private set
    }
}

class HandlerScheduler(private val handler: Handler) : Scheduler {
    override fun postDelayed(delayMs: Long, task: () -> Unit): Any {
        val runnable = Runnable { task() }
        handler.postDelayed(runnable, delayMs)
        return runnable
    }

    override fun cancel(handle: Any) {
        (handle as? Runnable)?.let(handler::removeCallbacks)
    }
}
