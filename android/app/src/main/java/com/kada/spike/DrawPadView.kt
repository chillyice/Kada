package com.kada.spike

import android.content.Context
import android.graphics.Canvas
import android.graphics.Paint
import android.graphics.Path
import android.graphics.PointF
import android.graphics.RectF
import android.view.MotionEvent
import android.view.View

/**
 * 画板：只负责收手指轨迹，**不认字、不执行**。
 *
 * P3 起一次手势可以有 1~[DrawStore.MAX_STROKES] 笔：抬手算一笔，笔迹留在板上，
 * 再按下就是下一笔。**笔画的先后顺序就是符号身份**（「圆 + 竖」≠「竖 + 圆」），
 * 所以这里只按顺序累积，不排序、不合并。
 *
 * 坐标在交付时归一化到 0~1 再交给 [onStroke]——识别器只认归一化坐标，
 * 这样「画板多大、屏幕多宽」都不影响结果（平板和手机上画的同一个符号是同一份点）。
 */
class DrawPadView(context: Context) : View(context) {

    /** 正在画的这一笔。 */
    private val points = ArrayList<PointF>()

    /** 已经画完的笔画（按先后顺序）。 */
    private val finished = ArrayList<List<PointF>>()

    private val stroke = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        color = 0xFFFF7A18.toInt()
        style = Paint.Style.STROKE
        strokeWidth = dp(4f)
        strokeCap = Paint.Cap.ROUND
        strokeJoin = Paint.Join.ROUND
    }

    private val board = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        color = 0xFF161616.toInt()
        style = Paint.Style.FILL
    }

    private val border = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        color = 0x33FFFFFF.toInt()
        style = Paint.Style.STROKE
        strokeWidth = dp(1f)
    }

    private val hint = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        color = 0xFF8A8A8A.toInt()
        textSize = dp(13f)
        textAlign = Paint.Align.CENTER
    }

    var hintText: String = "在这里画一个符号"

    /** 每一笔起手回调：画板据此取消「等下一笔」的定时器。 */
    var onStrokeBegin: (() -> Unit)? = null

    /** 一笔画完回调：**全部**笔画的归一化坐标（0~1，按画的先后顺序）。 */
    var onStroke: ((List<List<DollarOne.Pt>>) -> Unit)? = null

    /** 已经画完的笔画数（未抬手的这一笔不算）。 */
    val finishedStrokes: Int get() = finished.size

    override fun onDraw(canvas: Canvas) {
        super.onDraw(canvas)
        val rect = RectF(dp(1f), dp(1f), width - dp(1f), height - dp(1f))
        val radius = dp(16f)
        canvas.drawRoundRect(rect, radius, radius, board)
        canvas.drawRoundRect(rect, radius, radius, border)

        if (finished.isEmpty() && points.size < 2) {
            canvas.drawText(hintText, width / 2f, height / 2f, hint)
            return
        }
        val path = Path()
        for (line in finished) drawLine(path, line)
        drawLine(path, points)
        canvas.drawPath(path, stroke)
    }

    private fun drawLine(path: Path, line: List<PointF>) {
        if (line.size < 2) return
        path.moveTo(line[0].x, line[0].y)
        for (i in 1 until line.size) path.lineTo(line[i].x, line[i].y)
    }

    override fun onTouchEvent(event: MotionEvent): Boolean {
        when (event.actionMasked) {
            MotionEvent.ACTION_DOWN -> {
                // 已经够多了就忽略新的一笔：多出来的笔识别器不认，不如当场不动
                if (finished.size >= DrawStore.MAX_STROKES) return true
                parent?.requestDisallowInterceptTouchEvent(true)
                points.clear()
                points.add(PointF(event.x, event.y))
                onStrokeBegin?.invoke()
                invalidate()
                return true
            }

            MotionEvent.ACTION_MOVE -> {
                if (points.isEmpty()) return true
                points.add(PointF(event.x, event.y))
                invalidate()
                return true
            }

            MotionEvent.ACTION_UP -> {
                if (points.isEmpty()) return true
                points.add(PointF(event.x, event.y))
                // 一个点（误触）不成笔画：丢掉，别让它占掉一笔
                if (points.size >= 2) finished.add(ArrayList(points))
                points.clear()
                invalidate()
                parent?.requestDisallowInterceptTouchEvent(false)
                if (finished.isNotEmpty()) onStroke?.invoke(normalized())
                return true
            }

            MotionEvent.ACTION_CANCEL -> {
                points.clear()
                invalidate()
                parent?.requestDisallowInterceptTouchEvent(false)
                return true
            }
        }
        return true
    }

    private fun normalized(): List<List<DollarOne.Pt>> {
        val w = width.toFloat().coerceAtLeast(1f)
        val h = height.toFloat().coerceAtLeast(1f)
        return finished.map { line ->
            line.map { DollarOne.Pt((it.x / w).toDouble(), (it.y / h).toDouble()) }
        }
    }

    /** 清掉全部笔画（「重画」与识别失败后都用它）。 */
    fun clear() {
        points.clear()
        finished.clear()
        invalidate()
    }

    private fun dp(value: Float): Float = value * resources.displayMetrics.density
}
