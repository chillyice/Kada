package com.kada.spike

import android.content.Context
import android.graphics.Color
import android.graphics.Typeface
import android.view.Gravity
import android.widget.FrameLayout
import android.widget.LinearLayout
import android.widget.TextView

/**
 * 画板浮层（S2b）：**长按悬浮球**弹出来，画一个符号、抬手就执行。
 *
 * 四条边界：
 * 1. 只有**用户主动长按**才出现，不常驻、不自动弹——一个能盖住全屏的浮层，越安静越好；
 * 2. 点空白处 = 取消，画的笔画抬手后**先识别再执行**，认错方向宁可没反应；
 * 3. 执行走 `DrawStore.runSymbol` → `KeyFilterService.runShortcut`，与面板、音量键同一条动作链；
 * 4. P3 多笔画：抬手时如果「还有更长的符号对得上」（[DrawStore.mayContinue]），等 0.7 秒再提交，
 *    期间起新笔就取消定时器。**没有多笔画符号时恒为立即提交**——单笔画体验零回归。
 */
class DrawBoardView(context: Context, private val onClose: () -> Unit) : FrameLayout(context) {

    private companion object {
        /** 等下一笔的时长：够手指抬起来再落下，又不会让「就画一笔」感觉卡住。 */
        const val CONTINUE_DELAY_MS = 700L
        const val IDLE_TEXT = "画一个符号（1~3 笔）· 抬手执行 · 点空白处取消"
    }

    private val status: TextView
    private val pad: DrawPadView

    /** 正在等待下一笔的提交任务；起新笔 / 关闭浮层时取消。 */
    private var pending: Runnable? = null
    private var pendingStrokes: List<List<DollarOne.Pt>>? = null

    init {
        setBackgroundColor(0xCC000000.toInt())

        val column = LinearLayout(context).apply {
            orientation = LinearLayout.VERTICAL
            gravity = Gravity.CENTER
            setPadding(dp(16), dp(16), dp(16), dp(16))
        }

        status = TextView(context).apply {
            text = IDLE_TEXT
            setTextColor(Color.WHITE)
            textSize = 14f
            gravity = Gravity.CENTER
            setPadding(0, 0, 0, dp(10))
        }

        val side = (resources.displayMetrics.widthPixels * 0.78f).toInt()
            .coerceAtMost(dp(360))
            .coerceAtLeast(dp(220))
        pad = DrawPadView(context).apply {
            layoutParams = LinearLayout.LayoutParams(side, side)
            onStrokeBegin = {
                cancelPending()
                status.setTypeface(null, Typeface.NORMAL)
                status.text = IDLE_TEXT
            }
            onStroke = { strokes -> submit(strokes) }
        }

        val footer = TextView(context).apply {
            text = boundText()
            setTextColor(0xFF9E9E9E.toInt())
            textSize = 12f
            gravity = Gravity.CENTER
            setPadding(0, dp(10), 0, 0)
        }

        column.addView(status)
        column.addView(pad)
        column.addView(footer)
        addView(
            column,
            LayoutParams(LayoutParams.MATCH_PARENT, LayoutParams.MATCH_PARENT).apply {
                gravity = Gravity.CENTER
            },
        )

        // 点空白处（等于点这一层）取消；画板自己会吃掉落在它上面的触摸
        setOnClickListener { onClose() }
        column.setOnClickListener { onClose() }
    }

    /**
     * 一笔画完：可能还要接下一笔，也可能已经画完。
     *
     * 只在「真的存在更长的符号且前几笔对得上」时才等——没有多笔画符号的用户
     * 看到的行为与以前完全一样（抬手即执行）。
     */
    private fun submit(strokes: List<List<DollarOne.Pt>>) {
        if (DrawStore.mayContinue(strokes)) {
            cancelPending()
            status.setTypeface(null, Typeface.NORMAL)
            status.text = "还要再画一笔吗？不落笔就按这一笔画执行"
            val task = Runnable {
                pending = null
                val waiting = pendingStrokes
                pendingStrokes = null
                if (waiting != null) handle(waiting)
            }
            pending = task
            pendingStrokes = strokes
            postDelayed(task, CONTINUE_DELAY_MS)
            return
        }
        handle(strokes)
    }

    private fun handle(strokes: List<List<DollarOne.Pt>>) {
        cancelPending()
        val outcome = DrawStore.recognize(strokes)
        val symbol = outcome.symbol
        if (symbol == null) {
            status.text = outcome.reason
            pad.clear()
            return
        }
        val ok = DrawStore.runSymbol(symbol.id, "draw")
        if (ok) {
            val target = DrawStore.lastRun.substringAfter('→', DrawStore.lastRun)
            status.text = "✓ ${symbol.glyph} ${symbol.label} → $target"
            status.setTypeface(null, Typeface.BOLD)
            postDelayed({ onClose() }, 420)
        } else {
            status.text = "${symbol.glyph} ${symbol.label}：${DrawStore.lastError}"
            pad.clear()
        }
    }

    private fun cancelPending() {
        pending?.let { removeCallbacks(it) }
        pending = null
        pendingStrokes = null
    }

    override fun onDetachedFromWindow() {
        // 浮层关掉后定时器不能再跑：那会对着已消失的画板执行快捷键
        cancelPending()
        super.onDetachedFromWindow()
    }

    /** 底部一行：已经绑了哪些符号（没绑就告诉用户去哪儿绑）。 */
    private fun boundText(): String {
        val bindings = DrawStore.bindings()
        if (bindings.isEmpty()) {
            return "还没绑定符号：去应用内「画符号」区，把符号绑到一条快捷键"
        }
        return bindings.entries.joinToString(" · ") { (id, name) ->
            val glyph = DrawStore.symbolById(id)?.glyph ?: id
            "$glyph $name"
        }
    }

    private fun dp(value: Int): Int = (value * resources.displayMetrics.density).toInt()
}
