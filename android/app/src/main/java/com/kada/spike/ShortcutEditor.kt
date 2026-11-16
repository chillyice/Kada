package com.kada.spike

import android.app.Activity
import android.app.AlertDialog
import android.graphics.Typeface
import android.text.Editable
import android.text.TextWatcher
import android.view.Gravity
import android.view.View
import android.view.ViewGroup
import android.widget.ArrayAdapter
import android.widget.Button
import android.widget.CheckBox
import android.widget.EditText
import android.widget.LinearLayout
import android.widget.Spinner
import android.widget.TextView
import android.widget.Toast

/**
 * S1-d 最小配置编辑器：新建 / 编辑 / 删除一条快捷键，不碰桌面那套全量编辑器。
 *
 * 三条边界（踩过的地方）：
 * 1. **触发键的解析与撞车判定在 Rust**（`RustBridge.checkTrigger`）——界面只显示结果，
 *    不自己拆 `+` / `&` / 空格，否则手机上的判定与桌面分叉；
 * 2. **落盘走 `RustBridge.save`**（清洗 + 原子写 + 立即生效），不做「先写文件再等重启」；
 * 3. 动作链只编编辑器认得的五类，其余类型保持原样（`ACTION_KEEP`）——改个触发键不该把动作改没。
 *
 * 刷新口径：列表**只在用户动作后重建**（保存 / 删除 / 刷新 / 导入），不跟着主界面 500ms
 * 重绘跑——每半秒重建一遍控件既卡又会抢走输入焦点。
 */
class ShortcutEditor(private val activity: Activity) {

    private lateinit var listBox: LinearLayout
    private lateinit var tvSummary: TextView
    private var entries: List<ShortcutStore.Entry> = emptyList()
    private var rootView: View? = null

    /** 本面板的根视件（主界面「快捷键编辑器」按钮要滚到它）。 */
    fun view(): View? = rootView

    fun build(): View {
        val column = LinearLayout(activity).apply {
            orientation = LinearLayout.VERTICAL
            setPadding(0, dp(22), 0, 0)
        }
        rootView = column
        column.addView(title("S1 · 快捷键编辑器"))
        column.addView(
            hint(
                "改的是本机 config.json：保存即落盘、立即生效。触发键写法与桌面一致（F8 / Ctrl+Alt+K / " +
                    "F9 Space / F&J）；点「录入」可直接按键采集（组合键）。",
            ),
        )
        column.addView(
            row(
                button("新建快捷键") { editDialog(null) },
                button("刷新列表") { reloadList() },
            ),
        )
        tvSummary = body(11f)
        column.addView(tvSummary)
        listBox = LinearLayout(activity).apply { orientation = LinearLayout.VERTICAL }
        column.addView(listBox)
        reloadList()
        return column
    }

    /** 重新读配置并重建列表（导入 / 恢复出厂 / 保存后都要调）。 */
    fun reloadList() {
        if (!::listBox.isInitialized) return
        entries = ShortcutStore.entries()
        listBox.removeAllViews()
        if (entries.isEmpty()) {
            listBox.addView(body(12f).apply { text = "（配置里还没有快捷键）" })
        }
        for (entry in entries) listBox.addView(entryView(entry))
        tvSummary.text = buildString {
            append("配置里 ${entries.size} 条 · 启用 ${entries.count { it.enabled }} 条")
            val last = ShortcutStore.lastResult
            if (last.isNotEmpty()) append(" · 上次编辑：$last")
        }
    }

    // ------------------------------------------------------------------
    // 列表
    // ------------------------------------------------------------------

    private fun entryView(entry: ShortcutStore.Entry): View {
        val block = LinearLayout(activity).apply {
            orientation = LinearLayout.VERTICAL
            setPadding(0, dp(10), 0, dp(6))
        }
        val head = buildString {
            append("#${entry.index} ")
            append(entry.name.ifBlank { "(未命名)" })
            if (!entry.enabled) append(" ·（已停用）")
            if (entry.layer != null) append(" · 层 ${entry.layer}")
        }
        block.addView(TextView(activity).apply {
            text = head
            textSize = 14f
            setTypeface(typeface, Typeface.BOLD)
        })
        block.addView(body(12f, mono = true).apply {
            text = "触发键：" + (entry.triggers.joinToString("  |  ").ifBlank { "（无）" })
        })
        block.addView(body(12f).apply {
            text = "动作：" + entry.actionSummary
            setTextColor(0xFF444444.toInt())
        })
        block.addView(
            row(
                button("编辑") { editDialog(entry.index) },
                button("删除") { confirmDelete(entry) },
            ),
        )
        return block
    }

    // ------------------------------------------------------------------
    // 编辑对话框
    // ------------------------------------------------------------------

    private fun editDialog(index: Int?) {
        val entry = index?.let { i -> entries.firstOrNull { it.index == i } }
        val isNew = entry == null
        val excludeIndex = entry?.index ?: -1

        val column = LinearLayout(activity).apply {
            orientation = LinearLayout.VERTICAL
            setPadding(dp(20), dp(8), dp(20), dp(4))
        }

        fun label(text: String): TextView = TextView(activity).apply {
            this.text = text
            textSize = 12f
            setTypeface(typeface, Typeface.BOLD)
            setPadding(0, dp(10), 0, dp(2))
        }

        val nameField = EditText(activity).apply {
            hint = "名称（可留空）"
            textSize = 14f
            setSingleLine()
            setText(entry?.name ?: "")
        }
        column.addView(label("名称"))
        column.addView(nameField)

        val triggerField = EditText(activity).apply {
            hint = "F8 / Ctrl+Alt+K / F9 Space / F&J"
            textSize = 14f
            setSingleLine()
            setText(entry?.triggers?.firstOrNull() ?: "")
        }
        column.addView(label("触发键"))
        column.addView(triggerField)
        val tvCheck = body(11f)
        column.addView(tvCheck)
        column.addView(
            row(
                button("录入按键") { captureInto(triggerField, tvCheck, excludeIndex) },
                button("清空") { triggerField.setText("") },
            ),
        )
        if (entry != null && entry.triggers.size > 1) {
            column.addView(
                hint(
                    "这一条还有 ${entry.triggers.size - 1} 个触发键（" +
                        entry.triggers.drop(1).joinToString("、") + "）：编辑器只改第一个，其余原样保留。",
                ),
            )
        }

        val typeSpinner = Spinner(activity).apply {
            adapter = ArrayAdapter(
                activity,
                android.R.layout.simple_spinner_dropdown_item,
                ShortcutStore.actionTypes.map { ShortcutStore.actionLabel(it) },
            )
        }
        column.addView(label("动作类型"))
        column.addView(typeSpinner)

        val valueField = EditText(activity).apply {
            textSize = 14f
            setSingleLine()
            setText(entry?.actionValue ?: "")
        }
        column.addView(valueField)

        val coreOps = ActionCatalog.coreOps
        val opSpinner = Spinner(activity).apply {
            adapter = ArrayAdapter(
                activity,
                android.R.layout.simple_spinner_dropdown_item,
                coreOps.map { "${it.label}（${it.id}）" },
            )
        }
        column.addView(opSpinner)

        val enabledBox = CheckBox(activity).apply {
            text = "启用（关掉 = 保留但不生效）"
            textSize = 13f
            isChecked = entry?.enabled ?: true
        }
        column.addView(enabledBox)

        /** 动作类型切换：文本参数 / 语义动作下拉二选一显示。 */
        fun applyType() {
            val type = ShortcutStore.actionTypes[typeSpinner.selectedItemPosition]
            valueField.visibility =
                if (type == ShortcutStore.ACTION_KEEP || type == "semantic") View.GONE else View.VISIBLE
            opSpinner.visibility = if (type == "semantic") View.VISIBLE else View.GONE
            valueField.hint = when (type) {
                "open_url" -> "https://example.com"
                "text" -> "要输入到当前输入框的文本"
                "clipboard" -> "写入剪贴板的文本"
                "pause_ms" -> "毫秒（0 ~ 60000）"
                else -> ""
            }
            if (type == "semantic") {
                val op = entry?.actionValue.orEmpty()
                val pos = coreOps.indexOfFirst { it.id == op }
                if (pos >= 0) opSpinner.setSelection(pos)
            }
        }

        // 预选：编辑时用条目自己的类型；新建默认「打开网址」。
        val initialType = entry?.actionType ?: "open_url"
        typeSpinner.setSelection(ShortcutStore.actionTypes.indexOf(initialType).coerceAtLeast(0))
        applyType()
        typeSpinner.onItemSelectedListener = object : android.widget.AdapterView.OnItemSelectedListener {
            override fun onItemSelected(parent: android.widget.AdapterView<*>?, view: View?, position: Int, id: Long) =
                applyType()

            override fun onNothingSelected(parent: android.widget.AdapterView<*>?) = Unit
        }

        var updating = false

        fun refreshCheck() {
            if (updating) return
            val text = triggerField.text.toString().trim()
            if (text.isEmpty()) {
                tvCheck.text = "✗ 触发键不能为空"
                tvCheck.setTextColor(0xFFB3261E.toInt())
                return
            }
            val check = ShortcutStore.check(text, excludeIndex)
            when {
                !check.ok -> {
                    tvCheck.text = "✗ ${check.error}"
                    tvCheck.setTextColor(0xFFB3261E.toInt())
                }

                check.duplicates.isNotEmpty() -> {
                    tvCheck.text = "⚠ 与 ${check.duplicates.joinToString("、")} 撞同一触发键（core 判硬冲突，保存会被拦）"
                    tvCheck.setTextColor(0xFFB26A00.toInt())
                }

                else -> {
                    tvCheck.text = "✓ ${kindLabel(check.kind)}：${check.canonical}"
                    tvCheck.setTextColor(0xFF1B7F3B.toInt())
                }
            }
            // 大小写补正后回填正名（`ctrl+alt+k` → `Ctrl+Alt+K`）：只改一次，防抖靠 updating。
            if (check.ok && check.normalized && check.canonical != text) {
                updating = true
                triggerField.setText(check.canonical)
                triggerField.setSelection(check.canonical.length)
                updating = false
            }
        }

        triggerField.addTextChangedListener(object : TextWatcher {
            override fun beforeTextChanged(s: CharSequence?, start: Int, count: Int, after: Int) = Unit
            override fun onTextChanged(s: CharSequence?, start: Int, before: Int, count: Int) = Unit
            override fun afterTextChanged(s: Editable?) = refreshCheck()
        })

        val dialog = AlertDialog.Builder(activity)
            .setTitle(if (isNew) "新建快捷键" else "编辑 #${entry?.index}")
            .create()

        fun doSave() {
            val check = ShortcutStore.check(triggerField.text.toString(), excludeIndex)
            if (!check.ok) {
                Toast.makeText(activity, "触发键无效：${check.error}", Toast.LENGTH_LONG).show()
                return
            }
            if (check.duplicates.isNotEmpty()) {
                Toast.makeText(
                    activity,
                    "触发键与 ${check.duplicates.joinToString("、")} 重复，先改掉再保存",
                    Toast.LENGTH_LONG,
                ).show()
                return
            }
            val type = ShortcutStore.actionTypes[typeSpinner.selectedItemPosition]
            val value = if (type == "semantic") {
                coreOps.getOrNull(opSpinner.selectedItemPosition)?.id.orEmpty()
            } else {
                valueField.text.toString()
            }
            if (type != ShortcutStore.ACTION_KEEP && type != "semantic" && value.isBlank()) {
                Toast.makeText(activity, "「${ShortcutStore.actionLabel(type)}」还差参数", Toast.LENGTH_LONG).show()
                return
            }
            val result = ShortcutStore.save(
                index = entry?.index,
                name = nameField.text.toString(),
                trigger = check.canonical,
                actionType = type,
                actionValue = value,
                enabled = enabledBox.isChecked,
            )
            if (!result.ok) {
                Toast.makeText(activity, "保存失败：${result.error}", Toast.LENGTH_LONG).show()
                return
            }
            dialog.dismiss()
            reloadList()
            showSaveResult(result)
        }

        val buttons = LinearLayout(activity).apply {
            orientation = LinearLayout.HORIZONTAL
            gravity = Gravity.CENTER_VERTICAL
            setPadding(dp(20), dp(4), dp(20), dp(8))
            addView(
                button("取消") { dialog.dismiss() },
                LinearLayout.LayoutParams(0, ViewGroup.LayoutParams.WRAP_CONTENT, 1f),
            )
            addView(
                button("保存") { doSave() },
                LinearLayout.LayoutParams(0, ViewGroup.LayoutParams.WRAP_CONTENT, 1f),
            )
        }

        // 表单可滚（小屏放不下），按钮固定在底部：weight 在 wrap_content 对话框里会塌成 0，
        // 所以给 ScrollView 一个按屏高算出来的确定高度。
        val formHeight = (activity.resources.displayMetrics.heightPixels * 0.62).toInt()
        val root = LinearLayout(activity).apply { orientation = LinearLayout.VERTICAL }
        root.addView(
            android.widget.ScrollView(activity).apply { addView(column) },
            LinearLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, formHeight),
        )
        root.addView(buttons)

        dialog.setView(root)
        dialog.setOnDismissListener { TriggerCapture.cancel() }
        dialog.show()
        refreshCheck()
    }

    private fun showSaveResult(result: ShortcutStore.Save) {
        val problems = buildList {
            addAll(result.ignored)
            if (result.hardConflicts > 0) add("保存后仍有 ${result.hardConflicts} 条硬冲突（重复触发键等）")
        }
        if (problems.isEmpty()) {
            Toast.makeText(activity, "已保存：快捷键 ${result.shortcuts} 条", Toast.LENGTH_SHORT).show()
            return
        }
        AlertDialog.Builder(activity)
            .setTitle("已保存，但有提醒")
            .setMessage(problems.joinToString("\n"))
            .setPositiveButton("知道了", null)
            .show()
    }

    private fun confirmDelete(entry: ShortcutStore.Entry) {
        AlertDialog.Builder(activity)
            .setTitle("删除这条快捷键？")
            .setMessage(
                "#${entry.index} ${entry.name.ifBlank { "(未命名)" }}\n" +
                    "触发键：${entry.triggers.joinToString("、")}\n\n" +
                    "保存过的条目删除后只能从导出的配置或留档拿回来。",
            )
            .setNegativeButton("取消", null)
            .setPositiveButton("删除") { _, _ ->
                val result = ShortcutStore.delete(entry.index)
                if (!result.ok) {
                    Toast.makeText(activity, "删除失败：${result.error}", Toast.LENGTH_LONG).show()
                    return@setPositiveButton
                }
                reloadList()
                showSaveResult(result)
            }
            .show()
    }

    /**
     * 触发键录入：把下一下按键（含修饰键）采集进输入框。
     *
     * 采集期间所有按键都被无障碍层吞掉（[TriggerCapture]），Rust 判定整条跳过——
     * 否则录 F8 会先把 F8 的快捷键触发一遍。
     */
    private fun captureInto(field: EditText, tvCheck: TextView, excludeIndex: Int) {
        if (KeyFilterService.instance == null) {
            Toast.makeText(activity, "无障碍服务没连接：只能手敲触发键", Toast.LENGTH_LONG).show()
            return
        }
        TriggerCapture.begin { combo ->
            activity.runOnUiThread {
                field.setText(combo)
                field.setSelection(combo.length)
                val check = ShortcutStore.check(combo, excludeIndex)
                tvCheck.text = if (check.ok) "✓ 已录入：${check.canonical}" else "✗ ${check.error}"
                tvCheck.setTextColor(0xFF1B7F3B.toInt())
            }
        }
        Toast.makeText(activity, "请按一下要绑的键（可带 Ctrl / Alt / Shift / Meta）", Toast.LENGTH_LONG).show()
    }

    private fun kindLabel(kind: String): String = when (kind) {
        "combo" -> "组合键"
        "sequence" -> "按键序列"
        "chord" -> "和弦"
        else -> kind
    }

    // ------------------------------------------------------------------
    // 小工具（与 RustPanel 同一套观感）
    // ------------------------------------------------------------------

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
}
