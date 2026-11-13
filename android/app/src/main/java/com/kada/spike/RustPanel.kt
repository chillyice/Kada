package com.kada.spike

import android.app.Activity
import android.content.Intent
import android.graphics.Typeface
import android.net.Uri
import android.provider.Settings
import android.view.Gravity
import android.view.View
import android.view.ViewGroup
import android.widget.Button
import android.widget.CheckBox
import android.widget.LinearLayout
import android.widget.TextView
import android.widget.Toast
import org.json.JSONArray
import org.json.JSONObject
import java.text.SimpleDateFormat
import java.util.Date
import java.util.Locale

/**
 * S1 面板：配置驱动判定的状态、触发键清单、导入 / 导出（SAF）。
 *
 * 判定与配置全在 Rust（`crates/kada-android`）；这里**只展示**报告、只调 API，
 * 不解析触发键、不自己判支持不支持。
 */
class RustPanel(private val activity: Activity) {

    private lateinit var tvStatus: TextView
    private lateinit var tvTriggers: TextView
    private lateinit var tvLog: TextView
    private val timeFormat = SimpleDateFormat("HH:mm:ss", Locale.US)

    fun build(): View {
        val column = LinearLayout(activity).apply {
            orientation = LinearLayout.VERTICAL
            setPadding(0, dp(22), 0, 0)
        }
        column.addView(title("S1 · 配置驱动判定（Rust）"))
        column.addView(
            hint(
                "判定在 Rust（与桌面同一份 kada-core）：配置里 combos / sequences / chords 都按真规则跑。" +
                    "改键（remaps）与层在无 root 档不成立，只在报告里提示。",
            ),
        )

        column.addView(
            CheckBox(activity).apply {
                text = "用 Rust 判定（关掉 = S0 硬编码演示规则）"
                textSize = 13f
                isChecked = SpikeState.useRustEngine
                setOnCheckedChangeListener { _, value -> SpikeState.useRustEngine = value }
            },
        )

        column.addView(
            row(
                button("重载配置") { reload() },
                button("导入（合并）") { pickImport() },
                button("导出") { pickExport() },
            ),
        )

        tvStatus = body()
        column.addView(tvStatus)

        tvTriggers = body(11f, mono = true)
        column.addView(tvTriggers)

        tvLog = body(11f, mono = true)
        column.addView(tvLog)

        refresh()
        return column
    }

    fun refresh() {
        val report = RustBridge.json()
        tvStatus.text = if (report == null) {
            "Rust 未加载（libkada_android.so 缺失？）——已退回 S0 演示规则"
        } else if (!report.optBoolean("ok", false)) {
            "初始化失败：${report.optString("error")}"
        } else {
            statusText(report)
        }
        tvTriggers.text = triggersText()
        tvLog.text = logText()
    }

    private fun statusText(report: JSONObject): String {
        val counts = report.optJSONObject("counts") ?: JSONObject()
        val triggers = report.optJSONObject("triggers") ?: JSONObject()
        val unsupported = report.optJSONObject("unsupported") ?: JSONObject()
        val conflicts = report.optJSONArray("conflicts") ?: JSONArray()
        val errors = triggers.optJSONArray("errors") ?: JSONArray()
        val notes = report.optJSONArray("notes") ?: JSONArray()
        val warnings = report.optJSONArray("warnings") ?: JSONArray()

        var hard = 0
        var soft = 0
        for (i in 0 until conflicts.length()) {
            val c = conflicts.optJSONObject(i) ?: continue
            if (c.optString("severity") == "error") hard++ else soft++
        }

        return buildString {
            appendLine("库：kada_android ${RustBridge.version} · 配置 ${report.optString("path")}")
            appendLine(
                "快捷键 ${counts.optInt("shortcuts_enabled")}/${counts.optInt("shortcuts")} 启用 · " +
                    "组合 ${triggers.optInt("combo")} · 序列 ${triggers.optInt("sequence")} · " +
                    "和弦 ${triggers.optInt("chord")}",
            )
            appendLine(
                "本机不支持：改键 ${unsupported.optInt("remaps")} · 层内快捷键 ${unsupported.optInt("layer_shortcuts")} · " +
                    "热串 ${unsupported.optInt("expansions")} · 手势 ${unsupported.optInt("gestures") + unsupported.optInt("touchpad_gestures")}",
            )
            appendLine("冲突：${conflicts.length()}（硬 $hard / 软 $soft）· 触发键解析错误：${errors.length()}")
            if (warnings.length() > 0) {
                val first = warnings.optJSONObject(0)?.optString("summary").orEmpty()
                appendLine("配置告警：$first")
            }
            appendLine("判定次数：${SpikeState.rustDecisions}")
            for (i in 0 until notes.length()) {
                appendLine("· ${notes.optString(i)}")
            }
        }.trimEnd()
    }

    private fun triggersText(): String {
        val list = RustBridge.triggers() ?: return "（配置未加载）"
        if (list.length() == 0) return "（配置里没有启用的快捷键）"
        return buildString {
            appendLine("触发键：")
            for (i in 0 until list.length()) {
                val t = list.optJSONObject(i) ?: continue
                val kind = t.optString("kind")
                val mark = if (t.optBoolean("supported", false)) "✓" else "✗"
                val err = t.optString("error")
                val suffix = if (err.isNotEmpty()) "  !$err" else ""
                appendLine("$mark [${kind}] ${t.optString("trigger")}  ${t.optString("name")}$suffix")
            }
        }.trimEnd()
    }

    private fun logText(): String {
        val fires = SpikeState.rustFireLog().takeLast(6).reversed()
        if (fires.isEmpty()) return "（还没有按中任何快捷键）"
        return buildString {
            appendLine("最近触发：")
            for (f in fires) {
                val mark = if (f.ok) "✓" else "✗"
                appendLine("${timeFormat.format(Date(f.atMs))} $mark ${f.trigger} ${f.name}")
            }
        }.trimEnd()
    }

    private fun reload() {
        val report = RustBridge.reload()
        Toast.makeText(
            activity,
            if (report.isEmpty()) "Rust 未加载" else "已重载配置",
            Toast.LENGTH_SHORT,
        ).show()
        refresh()
    }

    fun pickImport() {
        val intent = Intent(Intent.ACTION_OPEN_DOCUMENT).apply {
            addCategory(Intent.CATEGORY_OPENABLE)
            type = "application/json"
        }
        runCatching { activity.startActivityForResult(intent, REQUEST_IMPORT) }
            .onFailure { Toast.makeText(activity, "没有文件选择器：${it.message}", Toast.LENGTH_LONG).show() }
    }

    fun pickExport() {
        val intent = Intent(Intent.ACTION_CREATE_DOCUMENT).apply {
            addCategory(Intent.CATEGORY_OPENABLE)
            type = "application/json"
            putExtra(Intent.EXTRA_TITLE, "kada-config.json")
        }
        runCatching { activity.startActivityForResult(intent, REQUEST_EXPORT) }
            .onFailure { Toast.makeText(activity, "没有文件选择器：${it.message}", Toast.LENGTH_LONG).show() }
    }

    /** 返回 true = 这个请求归本面板处理。 */
    fun onActivityResult(requestCode: Int, resultCode: Int, data: Intent?): Boolean {
        if (requestCode != REQUEST_IMPORT && requestCode != REQUEST_EXPORT) return false
        if (resultCode != Activity.RESULT_OK) return true
        val uri = data?.data ?: return true
        when (requestCode) {
            REQUEST_IMPORT -> importFrom(uri)
            REQUEST_EXPORT -> exportTo(uri)
        }
        return true
    }

    private fun importFrom(uri: Uri) {
        val text = runCatching {
            activity.contentResolver.openInputStream(uri)?.use { it.readBytes().decodeToString() }
        }.getOrNull()
        if (text.isNullOrBlank()) {
            Toast.makeText(activity, "读取文件失败", Toast.LENGTH_LONG).show()
            return
        }
        val report = RustBridge.importFrom(text, merge = true)
        val ok = report?.optBoolean("ok", false) ?: false
        val message = if (ok) {
            "已合并导入（只增不删；设置保留本机）"
        } else {
            "导入失败：${report?.optString("error") ?: "Rust 未加载"}"
        }
        Toast.makeText(activity, message, Toast.LENGTH_LONG).show()
        refresh()
    }

    private fun exportTo(uri: Uri) {
        val text = RustBridge.exportText()
        val ok = runCatching {
            activity.contentResolver.openOutputStream(uri)?.use { it.write(text.toByteArray()) }
            true
        }.getOrDefault(false)
        Toast.makeText(
            activity,
            if (ok) "已导出 ${text.length} 字节" else "写入失败",
            Toast.LENGTH_LONG,
        ).show()
    }

    // --- 小工具（与 MainActivity 同一套观感）---

    private fun dp(value: Int): Int = (value * activity.resources.displayMetrics.density).toInt()

    private fun title(text: String): TextView = TextView(activity).apply {
        this.text = text
        textSize = 16f
        setTypeface(typeface, Typeface.BOLD)
        setPadding(0, 0, 0, dp(6))
    }

    private fun hint(text: String): TextView = TextView(activity).apply {
        this.text = text
        textSize = 12f
        setTextColor(0xFF666666.toInt())
        setPadding(0, 0, 0, dp(6))
    }

    private fun body(sizeSp: Float = 13f, mono: Boolean = false): TextView = TextView(activity).apply {
        textSize = sizeSp
        if (mono) typeface = Typeface.MONOSPACE
        setPadding(0, dp(6), 0, dp(2))
        setLineSpacing(0f, 1.1f)
    }

    private fun button(text: String, onClick: () -> Unit): Button = Button(activity).apply {
        this.text = text
        textSize = 13f
        setOnClickListener { onClick() }
    }

    private fun row(vararg buttons: Button): LinearLayout = LinearLayout(activity).apply {
        orientation = LinearLayout.HORIZONTAL
        gravity = Gravity.CENTER_VERTICAL
        for (b in buttons) {
            addView(
                b,
                LinearLayout.LayoutParams(0, ViewGroup.LayoutParams.WRAP_CONTENT, 1f),
            )
        }
    }

    companion object {
        private const val REQUEST_IMPORT = 4101
        private const val REQUEST_EXPORT = 4102
    }
}
