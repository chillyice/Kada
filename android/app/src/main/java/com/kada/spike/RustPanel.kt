package com.kada.spike

import android.app.Activity
import android.app.AlertDialog
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
import java.io.File
import java.text.SimpleDateFormat
import java.util.Date
import java.util.Locale

/**
 * S1 面板：配置驱动判定的状态、触发键清单、导入 / 导出（SAF）。
 *
 * 判定与配置全在 Rust（`crates/kada-android`）；这里**只展示**报告、只调 API，
 * 不解析触发键、不自己判支持不支持。
 *
 * 导入有两条路，缺一不可：**合并**只增不删（往里加卡带），**替换**整份换掉
 * （把桌面那一份原样搬过来，示例 5 条随之消失；替换前 Rust 侧先留档）。
 */
class RustPanel(private val activity: Activity) {

    private lateinit var tvStatus: TextView
    private lateinit var tvTriggers: TextView
    private lateinit var tvLog: TextView
    private lateinit var tvImport: TextView
    private lateinit var tvUpdate: TextView
    private val editor = ShortcutEditor(activity)
    private val timeFormat = SimpleDateFormat("HH:mm:ss", Locale.US)

    /** 只想选一次文件，选择器回调时靠它区分合并 / 替换。 */
    private var pendingImportMerge: Boolean? = null

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
                button("导入·合并") { pickImport(merge = true) },
                button("导入·替换") { pickImport(merge = false) },
            ),
        )
        column.addView(
            row(
                button("导出") { pickExport() },
                button("恢复出厂") { confirmReset() },
            ),
        )

        tvImport = body(11f)
        column.addView(tvImport)

        // S1-f 自更新：侧载版没有商店，只能清单检查 + 下载 + 系统安装器确认。
        column.addView(
            row(
                button("检查更新") { checkUpdate() },
                button("安装已下载") { installUpdate() },
            ),
        )
        tvUpdate = body(11f)
        column.addView(tvUpdate)

        tvStatus = body()
        column.addView(tvStatus)

        tvTriggers = body(11f, mono = true)
        column.addView(tvTriggers)

        tvLog = body(11f, mono = true)
        column.addView(tvLog)

        // S1-d 编辑器挂在本面板末尾：列表只在用户动作后重建，不跟着主界面 500ms 重绘跑。
        column.addView(editor.build())

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
        if (::tvUpdate.isInitialized) tvUpdate.text = updateText()
    }

    /** 更新区的一行字：本机版本 + 最近一次检查 / 下载 / 安装的结果。 */
    private fun updateText(): String = buildString {
        append("版本 ${Update.currentName(activity)}（code ${Update.currentCode(activity)}）")
        append(" · 安装权限：${if (Update.canInstall(activity)) "允许" else "未允许"}")
        appendLine()
        append("[${Update.phase}] ${Update.lastResult.ifBlank { if (Update.phase == "idle") "还没检查过更新" else Update.phase }}")
        Update.localApk(activity)?.let {
            appendLine()
            append("本地安装包：${it.length() / 1024} KB（${it.name}）")
        }
        Update.lastCheck?.let { info ->
            appendLine()
            append("清单：${info.versionName}（code ${info.versionCode}）${if (Update.newer) " · 有新版本" else ""}")
        }
    }

    private fun checkUpdate() {
        tvUpdate.text = "检查中…"
        Thread {
            val result = Update.check(activity)
            activity.runOnUiThread {
                tvUpdate.text = updateText()
                val info = result.getOrNull()
                when {
                    result.isFailure || info == null ->
                        Toast.makeText(activity, Update.lastResult, Toast.LENGTH_LONG).show()

                    !Update.newer ->
                        Toast.makeText(activity, Update.lastResult, Toast.LENGTH_SHORT).show()

                    else -> confirmDownload(info)
                }
            }
        }.start()
    }

    private fun confirmDownload(info: Update.Info) {
        AlertDialog.Builder(activity)
            .setTitle("发现新版本 ${info.versionName}")
            .setMessage(
                buildString {
                    append("当前：${Update.currentName(activity)}（code ${Update.currentCode(activity)}）\n")
                    append("新版本：${info.versionName}（code ${info.versionCode}）\n")
                    if (info.notes.isNotBlank()) append("\n${info.notes}\n")
                    append("\n下载后由系统安装器确认；没授权「安装未知应用」会先引导去设置。")
                },
            )
            .setNegativeButton("取消", null)
            .setPositiveButton("下载并安装") { _, _ -> downloadAndInstall(info) }
            .show()
    }

    private fun downloadAndInstall(info: Update.Info) {
        tvUpdate.text = "下载中…"
        Thread {
            val result = Update.download(activity, info)
            activity.runOnUiThread {
                tvUpdate.text = updateText()
                if (result.isFailure) {
                    Toast.makeText(activity, Update.lastResult, Toast.LENGTH_LONG).show()
                } else {
                    installUpdate()
                }
            }
        }.start()
    }

    private fun installUpdate() {
        val result = Update.install(activity)
        if (::tvUpdate.isInitialized) tvUpdate.text = updateText()
        if (result.isFailure && Update.phase == "need_permission") {
            askInstallPermission()
        } else if (result.isFailure) {
            Toast.makeText(activity, Update.lastResult, Toast.LENGTH_LONG).show()
        }
    }

    private fun askInstallPermission() {
        AlertDialog.Builder(activity)
            .setTitle("需要允许安装未知应用")
            .setMessage(
                "系统不允许应用自己装自己：要先在设置里给 Kada 打开「安装未知应用」。\n\n" +
                    "点「去设置」跳到那一页，打开开关后回来再点「安装已下载」。",
            )
            .setNegativeButton("取消", null)
            .setPositiveButton("去设置") { _, _ -> Update.openInstallPermission(activity) }
            .show()
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
        editor.reloadList()
    }

    /** 主界面「快捷键编辑器」按钮用：把编辑器的根视件交出去，好滚到它。 */
    fun editorView(): View? = editor.view()

    /** 探针用：外部改完配置后让编辑器列表跟上（不重建整屏）。 */
    fun editorReloadForTest() {
        editor.reloadList()
    }

    fun pickImport(merge: Boolean) {
        if (merge) {
            openImportPicker(merge = true)
            return
        }
        AlertDialog.Builder(activity)
            .setTitle("替换导入")
            .setMessage(
                "会整份换掉本机配置：出厂示例 5 条与当前条目全部消失，改成你选的这一份。\n\n" +
                    "替换前 Rust 侧会先留档 config.json.pre-import-<时间戳>，选错了能从留档捞回来。",
            )
            .setNegativeButton("取消", null)
            .setPositiveButton("替换") { _, _ -> openImportPicker(merge = false) }
            .show()
    }

    private fun openImportPicker(merge: Boolean) {
        pendingImportMerge = merge
        val intent = Intent(Intent.ACTION_OPEN_DOCUMENT).apply {
            addCategory(Intent.CATEGORY_OPENABLE)
            // 只给 application/json 会让某些文件提供方（下载 / 网盘）把 .json 标成
            // octet-stream 后选不了，所以放开 */* 再限定几类常见 MIME。
            type = "*/*"
            putExtra(
                Intent.EXTRA_MIME_TYPES,
                arrayOf("application/json", "text/plain", "application/octet-stream"),
            )
        }
        runCatching { activity.startActivityForResult(intent, REQUEST_IMPORT) }
            .onFailure {
                pendingImportMerge = null
                Toast.makeText(activity, "没有文件选择器：${it.message}", Toast.LENGTH_LONG).show()
            }
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
        if (requestCode == REQUEST_IMPORT) {
            val merge = pendingImportMerge ?: true
            pendingImportMerge = null
            if (resultCode != Activity.RESULT_OK) return true
            val uri = data?.data ?: return true
            importFrom(uri, merge)
            return true
        }
        if (resultCode != Activity.RESULT_OK) return true
        val uri = data?.data ?: return true
        exportTo(uri)
        return true
    }

    private fun importFrom(uri: Uri, merge: Boolean) {
        val text = runCatching {
            activity.contentResolver.openInputStream(uri)?.use { it.readBytes().decodeToString() }
        }.getOrNull()
        if (text.isNullOrBlank()) {
            showImportResult("读取文件失败（${uri.lastPathSegment ?: uri}）", failed = true)
            return
        }
        val before = shortcutCount()
        val report = RustBridge.importFrom(text, merge)
        val ok = report?.optBoolean("ok", false) ?: false
        if (!ok) {
            val error = report?.optString("error").orEmpty().ifBlank { "Rust 未加载" }
            showImportResult("${if (merge) "合并" else "替换"}导入失败：$error", failed = true)
            refresh()
            return
        }
        val after = shortcutCount()
        val archive = if (merge) null else latestArchive()
        showImportResult(
            buildString {
                append("${if (merge) "合并" else "替换"}导入完成：快捷键 $before → $after 条")
                if (merge) append("（只增不删，设置保留本机）") else if (archive != null) append("\n旧配置留档：$archive")
            },
            failed = false,
        )
        refresh()
        editor.reloadList()
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

    private fun confirmReset() {
        AlertDialog.Builder(activity)
            .setTitle("恢复出厂示例")
            .setMessage(
                "会删掉本机 config.json，重新落回出厂 5 条示例。\n\n" +
                    "删之前先留档 config.json.pre-reset-<时间戳>。",
            )
            .setNegativeButton("取消", null)
            .setPositiveButton("恢复") { _, _ -> resetToSample() }
            .show()
    }

    private fun resetToSample() {
        val archive = archiveCurrent(PRE_RESET_MARK)
        RustBridge.resetToSample(activity)
        showImportResult(
            buildString {
                append("已恢复出厂示例：快捷键 ${shortcutCount()} 条")
                if (archive != null) append("\n旧配置留档：$archive")
            },
            failed = false,
        )
        refresh()
        editor.reloadList()
    }

    /**
     * 「恢复出厂」前把当前配置留一份：`RustBridge.resetToSample` 直接删文件，
     * 不像替换导入那样自带 `archive_copy`，这里补上同一个命名习惯，避免一键丢配置。
     */
    private fun archiveCurrent(mark: String): String? {
        val dir = activity.filesDir
        val source = File(dir, CONFIG_NAME)
        if (!source.exists()) return null
        val name = "$CONFIG_NAME.$mark-${ARCHIVE_STAMP.format(Date())}"
        return runCatching {
            source.copyTo(File(dir, name), overwrite = false)
            name
        }.getOrNull()
    }

    private fun latestArchive(): String? = activity.filesDir
        .listFiles { f -> f.name.startsWith("$CONFIG_NAME.$PRE_IMPORT_MARK-") }
        ?.maxByOrNull { it.lastModified() }
        ?.name

    private fun shortcutCount(): Int =
        RustBridge.json()?.optJSONObject("counts")?.optInt("shortcuts", -1) ?: -1

    private fun showImportResult(message: String, failed: Boolean) {
        if (!::tvImport.isInitialized) return
        tvImport.text = message
        tvImport.setTextColor(if (failed) 0xFFB3261E.toInt() else 0xFF1B7F3B.toInt())
        Toast.makeText(activity, message.replace("\n", "；"), Toast.LENGTH_LONG).show()
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
        private const val CONFIG_NAME = "config.json"
        private const val PRE_RESET_MARK = "pre-reset"
        private const val PRE_IMPORT_MARK = "pre-import"
        private val ARCHIVE_STAMP = SimpleDateFormat("yyyyMMdd-HHmmss", Locale.US)
    }
}
