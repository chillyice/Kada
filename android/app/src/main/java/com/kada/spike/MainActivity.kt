package com.kada.spike

import android.Manifest
import android.app.Activity
import android.content.Intent
import android.graphics.Typeface
import android.os.Build
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.provider.Settings
import android.view.Gravity
import android.view.View
import android.view.ViewGroup
import android.view.accessibility.AccessibilityManager
import android.view.inputmethod.InputMethodManager
import android.text.Editable
import android.text.TextWatcher
import android.util.Log
import android.widget.Button
import android.widget.CheckBox
import android.widget.EditText
import android.widget.LinearLayout
import android.widget.ScrollView
import android.widget.SeekBar
import android.widget.TextView
import android.widget.Toast
import java.text.SimpleDateFormat
import java.util.Date
import java.util.Locale

/**
 * S0 验收台：状态 / 实验开关 / 语义动作逐条测 / 统计 / 延迟 / 事件流 / 报告导出。
 *
 * 界面只为探针服务（programmatic View，没有 Compose、没有 androidx），一切以
 * 「能不能在真机上一眼判通过」为准。
 */
class MainActivity : Activity() {

    private val handler = Handler(Looper.getMainLooper())
    private val timeFormat = SimpleDateFormat("HH:mm:ss.SSS", Locale.US)
    private val actionResultViews = LinkedHashMap<String, TextView>()

    private lateinit var tvStatus: TextView
    private lateinit var tvStats: TextView
    private lateinit var tvLatency: TextView
    private lateinit var tvEvents: TextView
    private lateinit var tvReport: TextView
    private lateinit var field: EditText
    private lateinit var tvField: TextView
    private lateinit var rustPanel: RustPanel
    private lateinit var scroll: ScrollView

    private val refresh = object : Runnable {
        override fun run() {
            render()
            handler.postDelayed(this, 500)
        }
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        SpikeState.init(this)
        RustBridge.init(this)
        rustPanel = RustPanel(this)
        if (Build.VERSION.SDK_INT >= 33) {
            requestPermissions(arrayOf(Manifest.permission.POST_NOTIFICATIONS), 1)
        }
        setContentView(buildUi())
    }

    override fun onActivityResult(requestCode: Int, resultCode: Int, data: Intent?) {
        super.onActivityResult(requestCode, resultCode, data)
        if (rustPanel.onActivityResult(requestCode, resultCode, data)) {
            render()
        }
    }

    override fun onResume() {
        super.onResume()
        instance = this
        handler.removeCallbacks(refresh)
        handler.post(refresh)
    }

    override fun onPause() {
        super.onPause()
        handler.removeCallbacks(refresh)
    }

    override fun onDestroy() {
        handler.removeCallbacks(refresh)
        if (instance === this) instance = null
        super.onDestroy()
    }

    /**
     * 探针用：把光标放进「吞键验证输入框」，并滚到它可见。
     *
     * 自动化脚本用它代替「uiautomator 找控件 + input tap」——本界面每 500ms 刷新一次文本，
     * 窗口永远不空闲，uiautomator 会间歇性 dump 失败（点了半天没焦点，打字全丢）。
     *
     * 结果写进 `SpikeState.lastFocusAttempt` / `fieldFocused`：脚本据此判「是没聚焦还是真吞了」。
     */
    fun focusFieldForTest(attempt: Int = 0) {
        val imm = getSystemService(INPUT_METHOD_SERVICE) as InputMethodManager
        val requested = field.requestFocus()
        if (field.isFocused) {
            // 光标放到末尾：DEL 清空 / 追加键入都从尾部走，和用户的直觉一致。
            field.setSelection(field.text.length)
            scroll.post { scroll.smoothScrollTo(0, field.top) }
            imm.showSoftInput(field, InputMethodManager.SHOW_IMPLICIT)
            SpikeState.lastFocusAttempt =
                "ok attempt=$attempt request=$requested windowFocus=${hasWindowFocus()} ime=${imm.isActive(field)}"
            Log.i(SpikeState.TAG, "focusField ${SpikeState.lastFocusAttempt}")
            return
        }
        // 焦点有时要等窗口/输入法动画结束才给得出来：重试几次，别让脚本白等。
        if (attempt < 5) {
            handler.postDelayed({ focusFieldForTest(attempt + 1) }, 200)
            return
        }
        SpikeState.lastFocusAttempt =
            "failed attempt=$attempt request=$requested windowFocus=${hasWindowFocus()} visible=${field.isShown}"
        Log.i(SpikeState.TAG, "focusField ${SpikeState.lastFocusAttempt}")
    }

    override fun onWindowFocusChanged(hasFocus: Boolean) {
        super.onWindowFocusChanged(hasFocus)
        SpikeState.windowFocused = hasFocus
    }

    // ---------------------------------------------------------------------
    // 界面
    // ---------------------------------------------------------------------

    private fun buildUi(): View {
        val column = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            setPadding(dp(16), dp(16), dp(16), dp(32))
        }

        column.addView(textView("Kada Android S0", 22f, bold = true))
        column.addView(textView("真机验收台 · docs/安卓版规划.md §6 / §7", 12f).apply {
            setTextColor(0xFF666666.toInt())
            setPadding(0, dp(2), 0, dp(8))
        })

        tvStatus = body()
        column.addView(tvStatus)

        column.addView(
            hRow(
                button("无障碍设置") { startActivity(Intent(Settings.ACTION_ACCESSIBILITY_SETTINGS)) } to 1f,
                button("启动心跳") { startForegroundService(Intent(this, HeartbeatService::class.java)) } to 1f,
                button("停止心跳") { stopService(Intent(this, HeartbeatService::class.java)) } to 1f,
            ),
        )

        column.addView(section("触发实验"))
        column.addView(textView("演示规则", 13f, bold = true).apply { setPadding(0, dp(2), 0, dp(2)) })
        column.addView(textView(
            "F8 → 气泡（单键）\n" +
                "F7 → 复制（单键，需输入框有焦点）\n" +
                "F9 短按 → 气泡 / 长按 → 最近任务（tap-hold）\n" +
                "Ctrl+Alt+K → 通知栏（和弦）\n" +
                "音量上+音量下 → 返回（勾选后生效）",
            12f,
        ).apply { setTextColor(0xFF444444.toInt()) })

        column.addView(
            toggle("总闸：吞键（关掉 = 全部原样放行）", SpikeState.swallowMaster) {
                SpikeState.swallowMaster = it
            },
        )
        column.addView(
            toggle("演示规则（F7 / F8 / F9 / Ctrl+Alt+K）", SpikeState.demoRules) {
                SpikeState.demoRules = it
            },
        )
        column.addView(
            toggle("吞掉 A 键（端到端吞键验证：输入框里按不出 a）", SpikeState.swallowA) {
                SpikeState.swallowA = it
            },
        )
        column.addView(
            toggle("吞掉音量键（验证系统还会不会调音量）", SpikeState.swallowVolume) {
                SpikeState.swallowVolume = it
            },
        )
        column.addView(
            toggle("双音量键和弦 → 返回（手机档物理触发源实验）", SpikeState.volumeChord) {
                SpikeState.volumeChord = it
            },
        )
        column.addView(
            toggle("和弦全吞式（成员键全吞，验证「补不回来」的代价）", SpikeState.chordSwallowAll) {
                SpikeState.chordSwallowAll = it
            },
        )
        column.addView(tapHoldControl())

        column.addView(section("吞键端到端验证"))
        column.addView(textView(
            "点进下面的输入框，用 `adb shell input text hello` 打字：勾上「吞掉 A 键」后 a 不应出现，其它字母照常。",
            12f,
        ).apply { setTextColor(0xFF666666.toInt()) })
        field = EditText(this).apply {
            hint = "吞键验证输入框"
            textSize = 14f
            setSingleLine()
            addTextChangedListener(object : TextWatcher {
                override fun beforeTextChanged(s: CharSequence?, start: Int, count: Int, after: Int) = Unit
                override fun onTextChanged(s: CharSequence?, start: Int, before: Int, count: Int) {
                    SpikeState.fieldText = s?.toString() ?: ""
                }

                override fun afterTextChanged(s: Editable?) = Unit
            })
            setOnFocusChangeListener { _, hasFocus ->
                SpikeState.fieldFocused = hasFocus
                Log.i(SpikeState.TAG, "field focus=$hasFocus textLen=${field.text.length}")
            }
        }
        column.addView(field)
        tvField = body(11f, mono = true)
        column.addView(tvField)

        column.addView(section("语义动作（无 root 档的改键替身）"))
        column.addView(textView(
            "每条单独点；needsInput 的动作先把光标点进任意输入框。结果写进报告，用来逐条标注 ROM 可用性。",
            12f,
        ).apply { setTextColor(0xFF666666.toInt()) })
        for (spec in ActionCatalog.specs) {
            column.addView(actionRow(spec))
        }

        column.addView(section("统计"))
        tvStats = body()
        column.addView(tvStats)

        column.addView(section("动作延迟"))
        tvLatency = body()
        column.addView(tvLatency)

        column.addView(section("最近按键事件"))
        tvEvents = body(11f, mono = true)
        column.addView(tvEvents)

        column.addView(section("报告"))
        column.addView(
            hRow(
                button("导出报告") { exportReport() } to 1f,
                button("清空统计") {
                    SpikeState.clear()
                    Toast.makeText(this, "已清空", Toast.LENGTH_SHORT).show()
                } to 1f,
            ),
        )
        tvReport = body(10f, mono = true)
        column.addView(tvReport)

        // S1 面板放最后：S0 的界面顺序保持原样（自动化脚本按老位置找输入框 / 按钮）。
        column.addView(rustPanel.build())

        column.addView(textView(
            "adb：\n" +
                "settings put secure enabled_accessibility_services com.kada.spike/.KeyFilterService\n" +
                "settings put secure accessibility_enabled 1\n" +
                "am broadcast -a com.kada.spike.CONTROL -n com.kada.spike/.ControlReceiver --es op export",
            10f,
            mono = true,
        ).apply {
            setTextColor(0xFF666666.toInt())
            setPadding(0, dp(18), 0, 0)
        })

        return ScrollView(this).apply {
            addView(column)
            scroll = this
        }
    }

    private fun actionRow(spec: ActionSpec): View {
        val result = body(11f).apply { text = "未试" }
        actionResultViews[spec.id] = result
        val title = "${spec.label}  ·  ${spec.api}" + if (spec.needsInput) "  ·  需输入框" else ""
        return LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            setPadding(0, dp(8), 0, 0)
            addView(textView(title, 13f, bold = true))
            addView(
                hRow(
                    button("测试") { fireAction(spec.id) } to 0f,
                    result to 1f,
                ),
            )
        }
    }

    private fun tapHoldControl(): View {
        val label = textView("长按阈值 ${SpikeState.tapHoldMs} ms", 13f)
        val seek = SeekBar(this).apply {
            max = 40
            progress = ((SpikeState.tapHoldMs - 100) / 10).coerceIn(0, 40)
        }
        seek.setOnSeekBarChangeListener(object : SeekBar.OnSeekBarChangeListener {
            override fun onProgressChanged(bar: SeekBar?, progress: Int, fromUser: Boolean) {
                val ms = 100 + progress * 10
                SpikeState.tapHoldMs = ms
                label.text = "长按阈值 $ms ms"
            }

            override fun onStartTrackingTouch(bar: SeekBar?) = Unit

            override fun onStopTrackingTouch(bar: SeekBar?) = Unit
        })
        return LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            setPadding(0, dp(10), 0, 0)
            addView(label)
            addView(seek)
        }
    }

    // ---------------------------------------------------------------------
    // 刷新
    // ---------------------------------------------------------------------

    private fun render() {
        val tier = TierProbe.probe(this)
        val a11y = when {
            SpikeState.a11yConnected -> "已连接（filterKeyEvents=${SpikeState.filterKeyEventsFlagged}）"
            accessibilityServiceEnabled() -> "已启用但未连接（系统设置里关掉再打开）"
            else -> "未启用"
        }
        val heartbeat = if (SpikeState.heartbeatRunning()) {
            val minutes = (android.os.SystemClock.elapsedRealtime() - SpikeState.heartbeatStartedElapsed) / 60_000.0
            "运行 %.1f 分钟 · 心跳 %d 次".format(minutes, SpikeState.heartbeatTicks)
        } else {
            "未启动"
        }
        val foreground = listOfNotNull(SpikeState.foregroundPackage, SpikeState.foregroundClass)
            .joinToString(" / ").ifBlank { "—" }

        tvStatus.text = buildString {
            appendLine("无障碍：$a11y")
            appendLine("前台服务：$heartbeat")
            appendLine("档位：${tier.tierLabel}")
            appendLine("  root=${tier.root} su=${tier.suAvailable} uinput=${tier.uinputWritable} 物理键盘=${tier.physicalKeyboard}")
            if (tier.keyboards.isNotEmpty()) {
                appendLine("  键盘：" + tier.keyboards.take(2).joinToString("；") + if (tier.keyboards.size > 2) " 等 ${tier.keyboards.size} 个" else "")
            }
            appendLine("输入设备：${tier.devices.size} 个（明细见报告）")
            appendLine("前台窗口：$foreground")
            append("设备：sdk ${Build.VERSION.SDK_INT} · Android ${Build.VERSION.RELEASE} · ${Build.MODEL}")
        }

        val stats = SpikeState.stats()
        tvStats.text = buildString {
            appendLine("按下 ${stats.down} · 抬起 ${stats.up} · 连发 ${stats.repeat} · 未配对抬起 ${stats.unmatchedUp}")
            appendLine("吞掉 ${stats.swallowed} · 放行 ${stats.passed} · 动作 ${stats.actionsFired}（失败 ${stats.actionsFailed}）")
            append("按键未抬起：${if (stats.heldKeys.isEmpty()) "无" else stats.heldKeys.joinToString("、")}")
        }

        val latency = SpikeState.latencySummary()
        tvLatency.text = if (latency.count == 0) {
            "还没有动作样本"
        } else {
            "样本 ${latency.count} · P50 %.2f ms · P95 %.2f ms · max %.2f ms · avg %.2f ms\n（口径：无障碍回调收到 → 动作开始执行）"
                .format(latency.p50, latency.p95, latency.max, latency.avg)
        }

        val events = SpikeState.events().takeLast(14).reversed()
        tvEvents.text = if (events.isEmpty()) {
            "（还没有按键事件）"
        } else {
            events.joinToString("\n") { event ->
                "%s %-6s %-12s sw=%s pipe=%dms %s".format(
                    timeFormat.format(Date(event.wallMs)),
                    event.kind,
                    event.key,
                    if (event.swallowed) "Y" else "n",
                    event.pipelineMs,
                    event.note,
                )
            }
        }

        val records = SpikeState.actionRecords()
        for ((id, view) in actionResultViews) {
            val record = records[id]
            view.text = when {
                record == null -> "未试"
                record.lastOk == true -> "✓ 成功 ${record.okCount} 次 · ${record.lastDetail}"
                else -> "✗ 失败 ${record.failCount} 次 · ${record.lastDetail}"
            }
            view.setTextColor(
                when {
                    record == null -> 0xFF666666.toInt()
                    record.lastOk == true -> 0xFF1B7F3B.toInt()
                    else -> 0xFFB3261E.toInt()
                },
            )
        }

        tvReport.text = SpikeState.lastReportPath?.let { "上次导出：$it" } ?: "还没导出（getExternalFilesDir/kada-s0-report.json）"
        tvField.text = "输入框内容：\"${SpikeState.fieldText}\"（长度 ${SpikeState.fieldText.length}）"
        rustPanel.refresh()
    }

    // ---------------------------------------------------------------------
    // 动作
    // ---------------------------------------------------------------------

    private fun fireAction(id: String) {
        val service = KeyFilterService.instance
        if (service == null) {
            Toast.makeText(this, "无障碍服务未连接", Toast.LENGTH_SHORT).show()
            return
        }
        service.dispatchForTest(id)
        Toast.makeText(this, "已触发 $id", Toast.LENGTH_SHORT).show()
    }

    private fun exportReport() {
        val file = SpikeReport.write(this)
        Toast.makeText(this, "报告：${file.absolutePath}", Toast.LENGTH_LONG).show()
        render()
    }

    private fun accessibilityServiceEnabled(): Boolean {
        val manager = getSystemService(AccessibilityManager::class.java)
        if (!manager.isEnabled) return false
        val enabled = Settings.Secure.getString(
            contentResolver,
            Settings.Secure.ENABLED_ACCESSIBILITY_SERVICES,
        ) ?: return false
        return enabled.split(':').any { it.startsWith(packageName) }
    }

    // ---------------------------------------------------------------------
    // 小工具
    // ---------------------------------------------------------------------

    private fun dp(value: Int): Int = (value * resources.displayMetrics.density).toInt()

    private fun textView(
        text: String,
        sizeSp: Float = 14f,
        bold: Boolean = false,
        mono: Boolean = false,
    ): TextView = TextView(this).apply {
        this.text = text
        textSize = sizeSp
        if (bold) setTypeface(typeface, Typeface.BOLD)
        if (mono) typeface = Typeface.MONOSPACE
    }

    private fun body(sizeSp: Float = 14f, mono: Boolean = false): TextView =
        textView("", sizeSp, mono = mono).apply {
            setPadding(0, dp(6), 0, dp(6))
            setLineSpacing(0f, 1.15f)
        }

    private fun section(text: String): TextView =
        textView(text, 16f, bold = true).apply { setPadding(0, dp(22), 0, dp(8)) }

    private fun button(text: String, onClick: () -> Unit): Button = Button(this).apply {
        this.text = text
        textSize = 13f
        setOnClickListener { onClick() }
    }

    private fun toggle(label: String, checked: Boolean, onChange: (Boolean) -> Unit): CheckBox =
        CheckBox(this).apply {
            text = label
            textSize = 13f
            isChecked = checked
            setPadding(0, dp(2), 0, dp(2))
            setOnCheckedChangeListener { _, value -> onChange(value) }
        }

    private fun hRow(vararg items: Pair<View, Float>): LinearLayout =
        LinearLayout(this).apply {
            orientation = LinearLayout.HORIZONTAL
            gravity = Gravity.CENTER_VERTICAL
            for ((view, weight) in items) {
                val params = if (weight <= 0f) {
                    LinearLayout.LayoutParams(
                        ViewGroup.LayoutParams.WRAP_CONTENT,
                        ViewGroup.LayoutParams.WRAP_CONTENT,
                    )
                } else {
                    LinearLayout.LayoutParams(0, ViewGroup.LayoutParams.WRAP_CONTENT, weight)
                }
                addView(view, params)
            }
        }

    companion object {
        /** 探针用：让 adb 控制入口能直接操作当前界面（focusField）。 */
        @Volatile
        var instance: MainActivity? = null
            private set
    }
}
