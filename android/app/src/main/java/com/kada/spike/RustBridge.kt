package com.kada.spike

import android.content.Context
import android.util.Log
import java.io.File
import org.json.JSONArray
import org.json.JSONObject

/**
 * Kotlin ↔ Rust 桥（S1-a）：配置读写 / 判定全在 `crates/kada-android` 里，这里只搬运。
 *
 * **不要在 Kotlin 里重写触发解析或判定**——那会与桌面版（同一份 `kada-core`）分叉；
 * 这里只做两件事：把 Android `KeyEvent` 映射成中性键名（[KeyNames]），把判定结果执行掉。
 */
object RustBridge {

    private const val LIB = "kada_android"

    @Volatile
    var available = false
        private set

    @Volatile
    var initialized = false
        private set

    @Volatile
    var version: String = ""

    /** 最近一次报告（JSON 文本，未初始化时为空）。 */
    @Volatile
    var report: String = ""

    private var loadError: String? = null

    fun ensureLoaded(): Boolean {
        if (available) return true
        loadError?.let { return false }
        return try {
            System.loadLibrary(LIB)
            available = true
            version = nativeVersion()
            Log.i(SpikeState.TAG, "rust lib loaded version=$version")
            true
        } catch (t: Throwable) {
            loadError = t.message ?: t.toString()
            Log.e(SpikeState.TAG, "rust lib load failed: $loadError")
            false
        }
    }

    /** 首次初始化：`filesDir` 里没有配置就落一份安卓示例（Rust 侧负责）。 */
    fun init(context: Context): String {
        if (!ensureLoaded()) return ""
        if (initialized) return report
        val tier = TierProbe.probe(context).tier.toString()
        report = nativeInit(context.filesDir.absolutePath, tier)
        initialized = true
        Log.i(SpikeState.TAG, "rust init tier=$tier report=${report.take(400)}")
        return report
    }

    fun reload(): String {
        if (!ensureLoaded()) return ""
        report = nativeReload()
        return report
    }

    /**
     * 探针用：删掉磁盘上的配置，让 Rust 重新落一份安卓示例（自动化脚本要可重跑）。
     *
     * `nativeInit` 里的 `ensure_config` 只在文件不存在时写示例，所以「删 + 再 init」等价于
     * 「恢复出厂配置」。**只在探针里用**——产品里没有这条路径。
     */
    fun resetToSample(context: Context): String {
        if (!ensureLoaded()) return ""
        val dir = context.filesDir
        File(dir, "config.json").delete()
        File(dir, "config.json.bak").delete()
        val tier = TierProbe.probe(context).tier.toString()
        report = nativeInit(dir.absolutePath, tier)
        initialized = true
        Log.i(SpikeState.TAG, "rust config reset to sample: ${report.take(300)}")
        return report
    }

    fun initReport(context: Context): JSONObject? = parseObject(init(context))

    fun json(): JSONObject? = parseObject(report)

    fun triggers(): JSONArray? = runCatching {
        if (!ensureLoaded()) null else JSONArray(nativeTriggers())
    }.getOrNull()

    fun importFrom(text: String, merge: Boolean): JSONObject? {
        if (!ensureLoaded()) return null
        report = nativeImport(text, merge)
        return parseObject(report)
    }

    fun exportText(): String = if (ensureLoaded()) nativeExport() else ""

    fun save(text: String): JSONObject? {
        if (!ensureLoaded()) return null
        report = nativeSave(text)
        return parseObject(report)
    }

    fun setForeground(pkg: String?) {
        if (!ensureLoaded()) return
        runCatching { nativeSetForeground(pkg ?: "") }
    }

    fun reset() {
        if (available) runCatching { nativeReset() }
    }

    fun tick() {
        if (available) runCatching { nativeTick() }
    }

    /** 判定一次按键（同步，无 IO）：返回吞不吞 + 要执行的动作。 */
    fun decide(keyName: String, down: Boolean, repeat: Boolean, mods: String): Decision? {
        if (!available) return null
        val text = runCatching { nativeDecide(keyName, down, repeat, mods) }.getOrNull()
            ?: return null
        val obj = parseObject(text) ?: return null
        val fired = obj.optJSONArray("fired") ?: JSONArray()
        val list = buildList {
            for (i in 0 until fired.length()) {
                val item = fired.optJSONObject(i) ?: continue
                add(
                    Fired(
                        name = item.optString("name"),
                        trigger = item.optString("trigger"),
                        actions = item.optJSONArray("actions") ?: JSONArray(),
                        unsupported = item.optJSONArray("unsupported").toStringList(),
                    ),
                )
            }
        }
        return Decision(
            swallow = obj.optBoolean("swallow", false),
            fired = list,
            note = obj.optString("note"),
        )
    }

    private fun parseObject(text: String?): JSONObject? = runCatching {
        if (text.isNullOrBlank()) null else JSONObject(text)
    }.getOrNull()

    private fun JSONArray?.toStringList(): List<String> {
        if (this == null) return emptyList()
        return buildList {
            for (i in 0 until length()) add(optString(i))
        }
    }

    data class Fired(
        val name: String,
        val trigger: String,
        val actions: JSONArray,
        val unsupported: List<String>,
    )

    data class Decision(val swallow: Boolean, val fired: List<Fired>, val note: String)

    // --- JNI（实现见 crates/kada-android/src/lib.rs）---

    private external fun nativeVersion(): String
    private external fun nativeInit(filesDir: String, tier: String): String
    private external fun nativeReport(): String
    private external fun nativeReload(): String
    private external fun nativeDecide(
        key: String,
        down: Boolean,
        repeat: Boolean,
        mods: String,
    ): String
    private external fun nativeTick()
    private external fun nativeReset()
    private external fun nativeSetForeground(pkg: String): String
    private external fun nativeTriggers(): String
    private external fun nativeImport(text: String, merge: Boolean): String
    private external fun nativeExport(): String
    private external fun nativeSave(text: String): String
}
