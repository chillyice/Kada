package com.kada.spike

import android.content.Context
import android.hardware.input.InputManager
import android.os.Build
import android.os.SystemClock
import android.view.InputDevice
import android.view.KeyEvent
import java.io.File
import java.util.concurrent.TimeUnit

/**
 * 运行时档位探测（docs/安卓版规划.md §2）：
 * `/dev/uinput` 可写 + `su` 可用 → 档 3；有物理键盘 → 档 2；否则档 1。
 *
 * 「有没有物理键盘」不用「设备名里有没有 keyboard」猜——模拟器把触屏、电源键都
 * 报成带 SOURCE_KEYBOARD 的设备（实测 virtio_input_multi_touch_* 全部命中）。
 * 判据改成「设备真的有声母键」：`hasKeys(A, Z, Enter)` 全真才算键盘。
 *
 * 只在界面上刷新时跑，10 秒缓存——root 探测会 fork 进程，别按帧调。
 */
object TierProbe {

    data class DeviceInfo(
        val id: Int,
        val name: String,
        val sources: Int,
        val virtual: Boolean,
        val hasAlphabetKeys: Boolean,
    )

    data class Result(
        val root: Boolean,
        val suAvailable: Boolean,
        val testKeys: Boolean,
        val uinputWritable: Boolean,
        val physicalKeyboard: Boolean,
        val keyboards: List<String>,
        val devices: List<DeviceInfo>,
        val tier: Int,
        val tierLabel: String,
    )

    private var cached: Result? = null
    private var cachedAt = 0L

    fun probe(context: Context, force: Boolean = false): Result {
        val now = SystemClock.elapsedRealtime()
        val hit = cached
        if (!force && hit != null && now - cachedAt < 10_000) return hit
        val fresh = doProbe(context)
        cached = fresh
        cachedAt = now
        return fresh
    }

    private fun doProbe(context: Context): Result {
        val su = suAvailable()
        val testKeys = Build.TAGS?.contains("test-keys") == true
        val uinput = File("/dev/uinput").let { it.exists() && it.canWrite() }
        val root = su || testKeys || uinput

        val keyboards = ArrayList<String>()
        val devices = ArrayList<DeviceInfo>()
        val inputManager = context.getSystemService(InputManager::class.java)
        for (id in inputManager.inputDeviceIds) {
            val device = inputManager.getInputDevice(id) ?: continue
            val keyboardSource = device.sources and InputDevice.SOURCE_KEYBOARD != 0
            val alphabet = device.hasKeys(
                KeyEvent.KEYCODE_A,
                KeyEvent.KEYCODE_Z,
                KeyEvent.KEYCODE_ENTER,
                KeyEvent.KEYCODE_SPACE,
            ).all { it }
            devices.add(
                DeviceInfo(
                    id = device.id,
                    name = device.name,
                    sources = device.sources,
                    virtual = device.isVirtual,
                    hasAlphabetKeys = alphabet,
                ),
            )
            if (keyboardSource && !device.isVirtual && alphabet) {
                keyboards.add("${device.name} (id=${device.id}, ${device.vendorId}:${device.productId})")
            }
        }
        val physical = keyboards.isNotEmpty()

        val tier = when {
            root -> 3
            physical -> 2
            else -> 1
        }
        val label = when (tier) {
            3 -> "档 3 · root（evdev / uinput 全保真）"
            2 -> "档 2 · 平板 / 外接键盘（无障碍硬件键全量）"
            else -> "档 1 · 手机（无 root，只有点按 / 音量键）"
        }
        return Result(root, su, testKeys, uinput, physical, keyboards, devices, tier, label)
    }

    private fun suAvailable(): Boolean = try {
        val process = ProcessBuilder("su", "-c", "id").redirectErrorStream(true).start()
        val finished = process.waitFor(600, TimeUnit.MILLISECONDS)
        if (!finished) {
            process.destroy()
            false
        } else {
            process.exitValue() == 0
        }
    } catch (t: Throwable) {
        false
    }
}
