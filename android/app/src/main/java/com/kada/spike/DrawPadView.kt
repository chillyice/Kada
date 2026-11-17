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
 * 坐标在抬手时归一化到 0~1 再交给 [onStroke]——识别器只认归一化坐标，
 * 这样「画板多大、屏幕多宽」都不影响结果（平板和手机上画的同一个符号是同一份点）。
 */
class DrawPadView(context: Context) : View(context) {

    private val points = ArrayList<PointF>()

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
        color = 0x33FFFFFF
        style = Paint.Style.STROKE
        strokeWidth = dp(1f)
    }

    private val hint = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        color = 0xFF8A8A8A.toInt()
        textSize = dp(13f)
        textAlign = Paint.Align.CENTER
    }

    var hintText: String = "在这里画一个符号"

    /** 抬手回调：归一化坐标（0~1）。 */
    var onStroke: ((List<DollarOne.Pt>) -> Unit)? = null

    override fun onDraw(canvas: Canvas) {
        super.onDraw(canvas)
        val rect = RectF(dp(1f), dp(1f), width - dp(1f), height - dp(1f))
        val radius = dp(16f)
        canvas.drawRoundRect(rect, radius, radius, board)
        canvas.drawRoundRect(rect, radius, radius, border)

        if (points.size < 2) {
            canvas.drawText(hintText, width / 2f, height / 2f, hint)
            return
        }
        val path = Path().apply {
            moveTo(points[0].x, points[0].y)
            for (i in 1 until points.size) lineTo(points[i].x, points[i].y)
        }
        canvas.drawPath(path, stroke)
    }

    override fun onTouchEvent(event: MotionEvent): Boolean {
        when (event.actionMasked) {
            MotionEvent.ACTION_DOWN -> {
                parent?.requestDisallowInterceptTouchEvent(true)
                points.clear()
                points.add(PointF(event.x, event.y))
                invalidate()
                return true
            }

            MotionEvent.ACTION_MOVE -> {
                points.add(PointF(event.x, event.y))
                invalidate()
                return true
            }

            MotionEvent.ACTION_UP -> {
                points.add(PointF(event.x, event.y))
                val w = width.toFloat().coerceAtLeast(1f)
                val h = height.toFloat().coerceAtLeast(1f)
                val normalized = points.map {
                    DollarOne.Pt((it.x / w).toDouble(), (it.y / h).toDouble())
                }
                invalidate()
                parent?.requestDisallowInterceptTouchEvent(false)
                onStroke?.invoke(normalized)
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

    fun clear() {
        points.clear()
        invalidate()
    }

    private fun dp(value: Float): Float = value * resources.displayMetrics.density
}
