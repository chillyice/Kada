package com.kada.spike

import android.os.SystemClock
import android.view.KeyEvent

/** 无障碍回调收到的一个按键事件（只在主线程构造 / 消费）。 */
data class RawKey(
    val recvNanos: Long,
    val recvUptimeMs: Long,
    val eventTimeMs: Long,
    val keyCode: Int,
    val action: Int,
    val repeat: Int,
    val metaState: Int,
    val deviceId: Int,
    val scanCode: Int,
    val source: Int,
) {
    val isDown: Boolean get() = action == KeyEvent.ACTION_DOWN
    val isUp: Boolean get() = action == KeyEvent.ACTION_UP
    val isRepeat: Boolean get() = isDown && repeat > 0

    val kind: String
        get() = when {
            isUp -> "up"
            isRepeat -> "repeat"
            else -> "down"
        }

    val keyName: String get() = KeyNames.name(keyCode)
    val modifierLabel: String get() = KeyNames.modifierSummary(metaState)

    /**
     * 系统收到 → 无障碍回调收到 的流水线延迟。
     * 这是 S0-3 的一半数据：我们只能测「到手之后」，系统内部排队只能靠 eventTime 反推。
     */
    val pipelineMs: Long get() = (recvUptimeMs - eventTimeMs).coerceAtLeast(0)

    companion object {
        fun from(event: KeyEvent, recvNanos: Long): RawKey = RawKey(
            recvNanos = recvNanos,
            recvUptimeMs = SystemClock.uptimeMillis(),
            eventTimeMs = event.eventTime,
            keyCode = event.keyCode,
            action = event.action,
            repeat = event.repeatCount,
            metaState = event.metaState,
            deviceId = event.deviceId,
            scanCode = event.scanCode,
            source = event.source,
        )
    }
}
