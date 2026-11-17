package com.kada.spike

import android.content.Context
import android.os.Build
import android.provider.Settings
import android.util.Log
import org.json.JSONArray
import org.json.JSONObject
import java.io.File
import java.text.SimpleDateFormat
import java.util.Date
import java.util.Locale

/**
 * S0 报告：一次导出 = 一份可贴进 `docs/安卓版规划.md` §11 的证据。
 *
 * 落在 `getExternalFilesDir()`（`/sdcard/Android/data/com.kada.spike/files/`），
 * 不需要任何存储权限，`adb pull` 直接拿走。
 */
object SpikeReport {

    private const val FILE_NAME = "kada-s0-report.json"

    fun write(context: Context): File {
        val json = build(context)
        val dir = context.getExternalFilesDir(null) ?: context.filesDir
        dir.mkdirs()
        val file = File(dir, FILE_NAME)
        file.writeText(json.toString(2))
        SpikeState.lastReportPath = file.absolutePath
        Log.i(SpikeState.TAG, "report written: ${file.absolutePath} bytes=${file.length()}")
        return file
    }

    private fun build(context: Context): JSONObject {
        val root = JSONObject()
        val timestamp = SimpleDateFormat("yyyy-MM-dd'T'HH:mm:ssZ", Locale.US).format(Date())
        root.put("reportVersion", 1)
        root.put("generatedAt", timestamp)
        root.put("stage", "S0-spike")

        root.put("app", JSONObject().apply {
            put("package", context.packageName)
            put("versionName", runCatching {
                context.packageManager.getPackageInfo(context.packageName, 0).versionName
            }.getOrNull() ?: "?")
        })

        root.put("device", JSONObject().apply {
            put("manufacturer", Build.MANUFACTURER)
            put("brand", Build.BRAND)
            put("model", Build.MODEL)
            put("release", Build.VERSION.RELEASE)
            put("sdk", Build.VERSION.SDK_INT)
            put("fingerprint", Build.FINGERPRINT)
            put("tags", Build.TAGS ?: "")
        })

        val tier = TierProbe.probe(context, force = true)
        root.put("tier", JSONObject().apply {
            put("tier", tier.tier)
            put("label", tier.tierLabel)
            put("root", tier.root)
            put("su", tier.suAvailable)
            put("testKeys", tier.testKeys)
            put("uinputWritable", tier.uinputWritable)
            put("physicalKeyboard", tier.physicalKeyboard)
            put("keyboards", JSONArray(tier.keyboards))
            val devices = JSONArray()
            for (device in tier.devices) {
                devices.put(JSONObject().apply {
                    put("id", device.id)
                    put("name", device.name)
                    put("sources", device.sources)
                    put("virtual", device.virtual)
                    put("alphabetKeys", device.hasAlphabetKeys)
                })
            }
            put("inputDevices", devices)
        })

        root.put("accessibility", JSONObject().apply {
            put("connected", SpikeState.a11yConnected)
            put("filterKeyEventsFlagged", SpikeState.filterKeyEventsFlagged)
            put("enabledServices", runCatching {
                Settings.Secure.getString(
                    context.contentResolver,
                    Settings.Secure.ENABLED_ACCESSIBILITY_SERVICES,
                )
            }.getOrNull() ?: "")
            put("accessibilityEnabled", runCatching {
                Settings.Secure.getInt(context.contentResolver, Settings.Secure.ACCESSIBILITY_ENABLED)
            }.getOrDefault(0))
            put("foregroundPackage", SpikeState.foregroundPackage ?: "")
            put("foregroundClass", SpikeState.foregroundClass ?: "")
        })

        root.put("foregroundService", JSONObject().apply {
            put("running", SpikeState.heartbeatRunning())
            put("ticks", SpikeState.heartbeatTicks)
            val minutes = if (SpikeState.heartbeatStartedElapsed == 0L) 0.0
            else (android.os.SystemClock.elapsedRealtime() - SpikeState.heartbeatStartedElapsed) / 60_000.0
            put("uptimeMinutes", round(minutes))
        })

        root.put("experiment", JSONObject().apply {
            put("swallowMaster", SpikeState.swallowMaster)
            put("demoRules", SpikeState.demoRules)
            put("swallowA", SpikeState.swallowA)
            put("swallowVolume", SpikeState.swallowVolume)
            put("volumeChord", SpikeState.volumeChord)
            put("chordSwallowAll", SpikeState.chordSwallowAll)
            put("tapHoldMs", SpikeState.tapHoldMs)
        })

        val stats = SpikeState.stats()
        root.put("stats", JSONObject().apply {
            put("down", stats.down)
            put("up", stats.up)
            put("repeat", stats.repeat)
            put("unmatchedUp", stats.unmatchedUp)
            put("swallowed", stats.swallowed)
            put("passed", stats.passed)
            put("actionsFired", stats.actionsFired)
            put("actionsFailed", stats.actionsFailed)
            put("heldKeysAtSnapshot", JSONArray(stats.heldKeys))
            // 吞键端到端的物证：吞 A 时这里应当是空串
            put("inputFieldText", SpikeState.fieldText)
            // 打字前焦点在不在：空字符串 + focused=false 说明是「没聚焦」，不是「吞键生效」
            put("inputFieldFocused", SpikeState.fieldFocused)
            put("windowFocused", SpikeState.windowFocused)
            put("lastFocusAttempt", SpikeState.lastFocusAttempt)
        })

        val latency = SpikeState.latencySummary()
        root.put("latency", JSONObject().apply {
            put("count", latency.count)
            put("p50ms", round(latency.p50))
            put("p95ms", round(latency.p95))
            put("maxMs", round(latency.max))
            put("avgMs", round(latency.avg))
            put("scope", "无障碍回调收到 → 动作开始执行")
        })

        val events = JSONArray()
        for (event in SpikeState.events()) {
            events.put(JSONObject().apply {
                put("wallMs", event.wallMs)
                put("kind", event.kind)
                put("key", event.key)
                put("keyCode", event.keyCode)
                put("repeat", event.repeat)
                put("modifiers", event.modifiers)
                put("deviceId", event.deviceId)
                put("pipelineMs", event.pipelineMs)
                put("swallowed", event.swallowed)
                put("note", event.note)
            })
        }
        root.put("events", events)

        val actions = JSONObject()
        for ((id, record) in SpikeState.actionRecords()) {
            actions.put(id, JSONObject().apply {
                put("lastOk", record.lastOk ?: false)
                put("lastDetail", record.lastDetail)
                put("lastReason", record.lastReason)
                put("lastAtMs", record.lastAtMs)
                put("okCount", record.okCount)
                put("failCount", record.failCount)
                put("lastFailDetail", record.lastFailDetail)
                put("lastFailAtMs", record.lastFailAtMs)
            })
        }
        root.put("actions", actions)

        root.put("structuralLimits", JSONArray(listOf(
            "onKeyEvent 必须同步回答吞不吞：没有「先吞下、凑不齐再回放」的窗口",
            "只能吞不能注入：吞错的键补不回来",
            "软键盘走 InputConnection，不产生 KeyEvent：打字触发在手机档不存在",
        )))

        root.put("rust", JSONObject().apply {
            put("loaded", RustBridge.available)
            put("version", RustBridge.version)
            put("useRustEngine", SpikeState.useRustEngine)
            put("decisions", SpikeState.rustDecisions)
            put("report", RustBridge.json() ?: JSONObject())
            put("triggers", RustBridge.triggers() ?: JSONArray())
            val fires = JSONArray()
            for (fire in SpikeState.rustFireLog()) {
                fires.put(JSONObject().apply {
                    put("atMs", fire.atMs)
                    put("trigger", fire.trigger)
                    put("name", fire.name)
                    put("ok", fire.ok)
                    put("detail", fire.detail)
                })
            }
            put("fires", fires)
        })

        // S1-d 编辑器：最近一次编辑 / 校验结果 + 编辑器眼里的条目（adb 断言读这里，
        // 与界面共用同一份 ShortcutStore，避免「脚本测的是另一条路」）。
        root.put("editor", JSONObject().apply {
            put("lastResult", ShortcutStore.lastResult)
            ShortcutStore.lastCheck?.let { check ->
                put("lastCheck", JSONObject().apply {
                    put("ok", check.ok)
                    put("canonical", check.canonical)
                    put("kind", check.kind)
                    put("error", check.error)
                    put("normalized", check.normalized)
                    put("duplicates", JSONArray(check.duplicates))
                })
            }
            val list = JSONArray()
            for (entry in ShortcutStore.entries()) {
                list.put(JSONObject().apply {
                    put("index", entry.index)
                    put("name", entry.name)
                    put("triggers", JSONArray(entry.triggers))
                    put("enabled", entry.enabled)
                    put("actionType", entry.actionType)
                    put("actionSummary", entry.actionSummary)
                    put("actionCount", entry.actionCount)
                })
            }
            put("shortcuts", list)
        })

        // S1-f 自更新：本机版本 + 最近一次检查 / 下载 / 安装的状态（adb 断言读这里）。
        root.put("update", JSONObject().apply {
            put("currentVersion", Update.currentName(context))
            put("currentCode", Update.currentCode(context))
            put("manifestUrl", Update.MANIFEST_URL)
            put("phase", Update.phase)
            put("lastResult", Update.lastResult)
            put("newer", Update.newer)
            put("progressKb", Update.progressKb)
            put("canInstall", Update.canInstall(context))
            put("apkBytes", Update.localApk(context)?.length() ?: 0)
            Update.lastCheck?.let { info ->
                put("lastCheck", JSONObject().apply {
                    put("versionName", info.versionName)
                    put("versionCode", info.versionCode)
                    put("url", info.url)
                    put("sha256", info.sha256)
                    put("notes", info.notes)
                })
            }
        })

        // S2 手机档：总闸 / 悬浮球 / 磁贴 / 通知动作（adb 断言读这里）。
        root.put("phone", JSONObject().apply {
            put("master", SpikeState.swallowMaster)
            put("lastMasterToggle", SpikeState.lastMasterToggle)
            put("bubbleRunning", BubbleService.running)
            put("bubblePermission", BubbleService.hasPermission(context))
            put("bubblePanel", BubbleService.panelVisible)
            put("bubbleBoard", BubbleService.boardVisible)
            put("bubbleLastRun", BubbleService.lastRun)
            put("bubbleLastError", BubbleService.lastError)
            put("bubbleEntries", ShortcutStore.entries().count { it.enabled })
            // S2b 画符号：识别器自检 + 绑定表 + 最近一次识别 / 执行
            put("draw", DrawStore.reportJson())
        })

        return root
    }

    private fun round(value: Double): Double = Math.round(value * 100.0) / 100.0
}
