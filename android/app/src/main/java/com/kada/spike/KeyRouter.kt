package com.kada.spike

import android.view.KeyEvent

/** 定时器抽象：tap-hold 的「按住到点」只能靠定时器落地（桌面版同理由 `Engine::tick` 承担）。 */
interface Scheduler {
    fun postDelayed(delayMs: Long, task: () -> Unit): Any
    fun cancel(handle: Any)
}

/**
 * S0 的极小判定器：单键 → 动作、tap-hold（短按 / 长按）、双键以上和弦。
 *
 * 与桌面版 `Engine` 的差别只有三条，正是 S0-2 要证伪的东西：
 * 1. Android 的 `onKeyEvent` **必须同步回答吞不吞**，没有「先吞下、凑不齐再回放」的窗口；
 * 2. 只能「吞」不能「注入」，所以吞错的键补不回来；
 * 3. 判定不使用 `Engine` 的 down/up 配对假设——这里只信 `onKeyEvent` 真给到的事件。
 *
 * 和弦因此有两种模式（可在界面上切）：
 * - 非破坏式：成员照常放行，**只吞凑齐那一下**；代价是 app 也看见了修饰键按下；
 * - 全吞式：成员全吞，凑不齐 = 这几个键被白吃掉（桌面版靠「回放」补救，这里没有）。
 */
class KeyRouter(
    private val scheduler: Scheduler,
    private val fire: (actionId: String, reason: String, recvNanos: Long) -> Unit,
) {
    private class ChordRule(val id: String, val groups: List<IntArray>, val action: String) {
        val members: Set<Int> = groups.flatMap { it.toList() }.toSet()
        fun complete(held: Set<Int>): Boolean = groups.all { group -> group.any { it in held } }
    }

    private val singleRules: Map<Int, String> = mapOf(
        KeyEvent.KEYCODE_F8 to "toast",
        KeyEvent.KEYCODE_F7 to "copy",
    )

    private val tapHoldKey = KeyEvent.KEYCODE_F9
    private val tapHoldTapAction = "toast"
    private val tapHoldHoldAction = "recents"

    private val modChord = ChordRule(
        id = "Ctrl+Alt+K",
        groups = listOf(
            intArrayOf(KeyEvent.KEYCODE_CTRL_LEFT, KeyEvent.KEYCODE_CTRL_RIGHT),
            intArrayOf(KeyEvent.KEYCODE_ALT_LEFT, KeyEvent.KEYCODE_ALT_RIGHT),
            intArrayOf(KeyEvent.KEYCODE_K),
        ),
        action = "notifications",
    )

    private val volumeChordRule = ChordRule(
        id = "VolumeUp+VolumeDown",
        groups = listOf(
            intArrayOf(KeyEvent.KEYCODE_VOLUME_UP),
            intArrayOf(KeyEvent.KEYCODE_VOLUME_DOWN),
        ),
        action = "back",
    )

    /** 物理按住集合（只用于和弦 completeness，不参与吞键决策）。 */
    private val held = HashSet<Int>()

    /** 按下被吞掉的键：抬起一起吞，别让 app 看到半截按键。 */
    private val swallowedDowns = HashSet<Int>()

    /** 本轮按下周期里已判定过的和弦（全成员抬起才清空），防连发重复触发。 */
    private val epochFired = HashSet<String>()

    private var holdHandle: Any? = null
    private var holdFired = false

    /** 返回 true = 吞掉（`onKeyEvent` 返回 true）。 */
    fun onKey(raw: RawKey): Boolean {
        trackHeld(raw)
        val note = StringBuilder()
        var swallow = decide(raw, note)
        if (raw.isDown) {
            if (swallow) swallowedDowns.add(raw.keyCode)
        } else if (swallowedDowns.remove(raw.keyCode)) {
            swallow = true
        }
        SpikeState.noteKey(raw, swallow, note.toString())
        return swallow
    }

    /** 服务断开 / 重连：与物理键盘断过一次线，所有等待态作废（架构设计 §3.9 的 Android 版）。 */
    fun shutdown() {
        holdHandle?.let { scheduler.cancel(it) }
        holdHandle = null
        holdFired = false
        held.clear()
        swallowedDowns.clear()
        epochFired.clear()
    }

    private fun trackHeld(raw: RawKey) {
        when {
            raw.isUp -> held.remove(raw.keyCode)
            raw.repeat == 0 -> held.add(raw.keyCode)
        }
    }

    private fun decide(raw: RawKey, note: StringBuilder): Boolean {
        // 0) 总闸关掉 = 所有按键原样放行（对照实验：确认吞键确实是我们干的）。
        if (!SpikeState.swallowMaster) return false

        // 1) 吞 A：端到端验证「无障碍吞键」的调试开关，不受演示规则总闸影响。
        if (SpikeState.swallowA && raw.keyCode == KeyEvent.KEYCODE_A) {
            note.append("swallowA")
            return true
        }

        // 1b) 吞音量键：验证「吞掉之后系统还会不会调音量」——手机档的命门。
        if (SpikeState.swallowVolume && KeyNames.isVolume(raw.keyCode)) {
            note.append("swallowVolume")
            return true
        }

        if (!SpikeState.demoRules) return false

        // 2) 单键 → 动作
        singleRules[raw.keyCode]?.let { action ->
            if (raw.isDown && raw.repeat == 0) {
                note.append("single:${raw.keyName}")
                fire(action, "rule:single:${raw.keyName}", raw.recvNanos)
            }
            return true
        }

        // 3) tap-hold
        if (raw.keyCode == tapHoldKey) {
            return tapHold(raw, note)
        }

        // 4) 和弦
        chord(raw, note)?.let { return it }

        return false
    }

    private fun tapHold(raw: RawKey, note: StringBuilder): Boolean {
        if (raw.isDown) {
            if (raw.repeat > 0) return true
            holdFired = false
            note.append("taphold:down")
            holdHandle = scheduler.postDelayed(SpikeState.tapHoldMs.toLong()) {
                holdHandle = null
                holdFired = true
                fire(tapHoldHoldAction, "rule:taphold:hold:${SpikeState.tapHoldMs}ms", raw.recvNanos)
            }
            return true
        }

        holdHandle?.let {
            scheduler.cancel(it)
            holdHandle = null
        }
        if (holdFired) {
            note.append("taphold:hold-end")
        } else {
            note.append("taphold:tap")
            fire(tapHoldTapAction, "rule:taphold:tap", raw.recvNanos)
        }
        holdFired = false
        return true
    }

    /** 返回 null = 这个键不属于任何和弦。 */
    private fun chord(raw: RawKey, note: StringBuilder): Boolean? {
        val rules = buildList {
            add(modChord)
            if (SpikeState.volumeChord) add(volumeChordRule)
        }.filter { raw.keyCode in it.members }
        if (rules.isEmpty()) return null

        val swallowAll = SpikeState.chordSwallowAll
        var swallow = false

        for (rule in rules) {
            if (raw.isDown) {
                val complete = rule.complete(held)
                val already = rule.id in epochFired
                if (swallowAll) swallow = true
                if (complete && !already && raw.repeat == 0) {
                    epochFired.add(rule.id)
                    if (!swallowAll) swallow = true
                    note.append("chord:${rule.id}")
                    fire(rule.action, "rule:chord:${rule.id}", raw.recvNanos)
                }
            } else {
                if (swallowAll) swallow = true
                // 全成员都抬起才算这一轮结束；只有全吞式才记「白吞了」的代价。
                if (rule.members.none { it in held }) {
                    val fired = rule.id in epochFired
                    epochFired.remove(rule.id)
                    if (swallowAll && !fired) note.append("chord-miss:${rule.id}")
                }
            }
        }
        return swallow
    }
}
