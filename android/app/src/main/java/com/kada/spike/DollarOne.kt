package com.kada.spike

import kotlin.math.abs
import kotlin.math.atan2
import kotlin.math.cos
import kotlin.math.hypot
import kotlin.math.max
import kotlin.math.roundToInt
import kotlin.math.sin
import kotlin.math.sqrt

/**
 * $1 Unistroke Recognizer（Wobbrock 等，UIST 2007）的 Kotlin 版，外加「方向判定」。
 *
 * 为什么用它而不是自己数角点 / 上模型：
 * - 纯计算、零依赖、单笔画，一次识别不到 1 ms（32 点 × 14 个形状类别）；
 * - 模板就是普通折线——预设符号与「以后用户自己录的画法」是同一张表，不用两套代码；
 * - 真机上「拐点 / 角点」这类特征对抖动极其敏感，$1 的重采样 + 归一化天然抗抖。
 *
 * 与论文原版的三处差异（都是为「四个方向的三角」这类需求服务的）：
 * 1. **旋转搜索放到整圈**：原版只在 ±45° 内微调，方向被写死在模板里；这里先做
 *    旋转无关的形状匹配，再把「最优旋转角 θ」拿去判方向。θ = 把画出来的笔画转回
 *    模板标准朝向所需的角度。
 * 2. **起点 / 笔顺不敏感**：闭合形状换个起点（三角形从另一个角起笔）或反着画，
 *    原版会直接不命中；这里对候选点序列做**反向**（所有形状）+ **循环移位**
 *    （仅闭合回路，开放折线移位后是另一条乱线）后再比，取最小距离。
 * 3. **方向按 90° 扇区量化**：θ 落在某扇区中心 ±[toleranceDeg] 内才算数；落在两
 *    扇区中间（约 45°）就返回「方向不明确」——**猜错方向比不识别更糟**。
 *
 * 坐标约定：屏幕坐标系（x 向右、y 向下）。角度用标准旋转矩阵，
 * 所以 `rotate(p, +90°)` 在屏幕上看起来是**顺时针**转 90°。
 */
object DollarOne {

    /** 重采样点数：原文 64，这里 32 够用（方向靠全局拟合，不靠细节），快一倍。 */
    const val N = 32

    /** 归一化后的方框边长（原文 250）；只影响分数分母，取整便于读日志。 */
    private const val SIZE = 200.0
    private val HALF_DIAGONAL = 0.5 * sqrt(2.0 * SIZE * SIZE)

    /**
     * 「距离差不多」的相对阈值：0.02 = 2%。
     * 只用来在 180° 旋转对称的形状（Z / N / S）里挑转角小的那个解；
     * 真正的分数比较仍走调用方的 [DrawStore] 阈值，这里不掺和。
     */
    private const val TIE = 0.02

    /**
     * 多笔画符号的**单笔下限**：总分是各笔平均，只卡总分的话「一笔完美 + 一笔乱涂」
     * 也能靠平均分混过去。0.72 与总分门槛 0.80 同档（画歪一点仍算数，乱涂不行）。
     */
    private const val STROKE_FLOOR = 0.72

    data class Pt(val x: Double, val y: Double)

    /** 一个形状模板：`cls` = 类别，`directional` = 方向是否有意义，`norm` = 归一化后的点。 */
    data class Template(
        val cls: String,
        val directional: Boolean,
        val norm: List<Pt>,
        /** 闭合形状才允许「换个起笔点」（循环移位）；开放折线移位后是另一条乱线。 */
        val closed: Boolean = false,
    )

    /**
     * 一个符号的模板：预设符号都是单笔画，自定义符号可以有 1~3 笔。
     *
     * 多笔画**按画的先后顺序**比（「圆 + 竖」与「竖 + 圆」是两个符号），每笔各自归一化，
     * 所以只比形状与顺序、不比相对位置和大小——随手画的时候人对位置的控制最差，
     * 拿位置当特征只会让同一符号每次都抖。
     */
    data class Shape(
        val cls: String,
        val directional: Boolean,
        val strokes: List<Template>,
    )

    data class Score(
        val cls: String,
        val score: Double,
        val thetaDeg: Double,
        val bucket: Int,
        val residualDeg: Double,
        val directional: Boolean,
        /** 方向落在扇区中心 ±容差内（无方向形状恒为 true）。 */
        val valid: Boolean,
    )

    data class Result(
        val cls: String,
        val bucket: Int,
        val score: Double,
        val thetaDeg: Double,
        /** 方向合法的候选，按分数降序；第一个就是命中。 */
        val ranked: List<Score>,
        /** 方向不合法（约 45° 夹角）而淘汰的候选——它常常正是分数最高的那个。 */
        val edgeRejected: List<Score>,
    ) {
        val second: Score? get() = ranked.getOrNull(1)

        val bestRejected: Score? get() = edgeRejected.firstOrNull()
    }

    fun template(
        cls: String,
        directional: Boolean,
        path: List<Pt>,
        closed: Boolean = false,
    ): Template? {
        val norm = normalize(path) ?: return null
        return Template(cls, directional, norm, closed)
    }

    /** 单笔画的形状（预设符号都走这条）。 */
    fun shape(cls: String, directional: Boolean, path: List<Pt>, closed: Boolean = false): Shape? =
        template(cls, directional, path, closed)?.let { Shape(cls, directional, listOf(it)) }

    /** 起笔与收笔几乎重合（不超过总长 15%）= 闭合回路，识别时允许换个起笔点。 */
    fun isClosed(points: List<Pt>): Boolean {
        val clean = points.filter { it.x.isFinite() && it.y.isFinite() }
        if (clean.size < 3) return false
        var total = 0.0
        for (i in 1 until clean.size) {
            total += hypot(clean[i].x - clean[i - 1].x, clean[i].y - clean[i - 1].y)
        }
        if (total < 1e-9) return false
        val first = clean.first()
        val last = clean.last()
        return hypot(last.x - first.x, last.y - first.y) <= total * 0.15
    }

    /**
     * 识别一串笔画（1~3 笔，按画的先后顺序）。`shapes` 由调用方建好（归一化只做一次）。
     *
     * - **单笔画预设**要求「只画了一笔」：多笔画手势不拿第一笔去猜三角——宁可没反应。
     * - **自定义符号**要求笔数相同，逐笔比形状，总分取各笔平均；另加单笔下限
     *   [STROKE_FLOOR]——否则「一笔画得完美 + 一笔乱涂」也能靠平均分混过去。
     * - 退化成点的笔画（误触的小点）直接丢掉，不算一笔。
     *
     * 返回 null = 没有一笔能用（点太少 / 长度为零），**不是**「认出来了但方向不对」——
     * 后者仍然返回 Result，`ranked` 为空、`edgeRejected` 非空。
     */
    fun recognize(
        strokes: List<List<Pt>>,
        shapes: List<Shape>,
        toleranceDeg: Double = 35.0,
    ): Result? {
        val drawn = strokes.mapNotNull { normalize(it) }
        if (drawn.isEmpty()) return null
        val scores = ArrayList<Score>(shapes.size)
        for (s in shapes) {
            if (s.strokes.size == 1) {
                // 单笔画符号（全部预设 + 用户录的一笔画）：多笔画手势不拿第一笔去猜，
                // 否则「圆 + 竖」的第一笔就被当成圆执行了。
                if (drawn.size != 1) continue
                val t = s.strokes[0]
                if (t.norm.size != N) continue
                val align = bestAlign(drawn[0], t.norm, t.closed) ?: continue
                val deg = degrees(align.theta)
                // 无方向形状（圆 / 菱形 / 星 / 勾）的 θ 不稳定：不判方向、恒算扇区 0
                val bucket = if (s.directional) bucketOf(deg) else 0
                val residual = if (s.directional) residualOf(deg, bucket) else 0.0
                scores.add(
                    Score(
                        cls = t.cls,
                        score = 1.0 - align.distance / HALF_DIAGONAL,
                        thetaDeg = deg,
                        bucket = bucket,
                        residualDeg = residual,
                        directional = s.directional,
                        valid = !s.directional || abs(residual) <= toleranceDeg,
                    ),
                )
            } else {
                if (drawn.size != s.strokes.size) continue
                val parts = drawn.indices.mapNotNull { i -> strokeScore(drawn[i], s.strokes[i]) }
                if (parts.size != drawn.size) continue
                scores.add(
                    Score(
                        cls = s.cls,
                        score = parts.average(),
                        thetaDeg = 0.0,
                        bucket = 0,
                        residualDeg = 0.0,
                        directional = false,
                        valid = parts.min() >= STROKE_FLOOR,
                    ),
                )
            }
        }
        val ranked = scores.filter { it.valid }.sortedByDescending { it.score }
        val edge = scores.filter { !it.valid }.sortedByDescending { it.score }
        val best = ranked.firstOrNull() ?: return null
        return Result(best.cls, best.bucket, best.score, best.thetaDeg, ranked, edge)
    }

    /**
     * 单笔画的便捷入口（自检、面板里的单笔用例都走它）。
     *
     * 名字不能也叫 `recognize`：`List<Pt>` 与 `List<List<Pt>>` 在 JVM 上都擦成 `List`，
     * 同名重载会撞签名（Kotlin 会报 Platform declaration clash）。
     */
    fun recognizeOne(
        points: List<Pt>,
        shapes: List<Shape>,
        toleranceDeg: Double = 35.0,
    ): Result? = recognize(listOf(points), shapes, toleranceDeg)

    /** 一笔与一个模板的分数；点太少 / 对齐失败返回 null。 */
    internal fun strokeScore(drawn: List<Pt>, t: Template): Double? {
        if (t.norm.size != N) return null
        val align = bestAlign(drawn, t.norm, t.closed) ?: return null
        return 1.0 - align.distance / HALF_DIAGONAL
    }

    /**
     * 归一化：重采样 32 点 → 以质心居中 → **等比**缩放到 [SIZE] 方框。
     *
     * 为什么是等比而不是原文的 `ScaleToSquare`（两轴各自拉满）：等比缩放与旋转可交换，
     * 转 90° 再归一化和归一化再转 90° 是同一个形状——方向判定靠这一点才稳。
     * 代价是「画得特别扁」的符号分数会低一点，那本来就该重画。
     */
    fun normalize(points: List<Pt>): List<Pt>? {
        val clean = points.filter { it.x.isFinite() && it.y.isFinite() }
        if (clean.size < 2) return null
        val resampled = resample(clean, N) ?: return null

        var minX = Double.MAX_VALUE
        var maxX = -Double.MAX_VALUE
        var minY = Double.MAX_VALUE
        var maxY = -Double.MAX_VALUE
        var sumX = 0.0
        var sumY = 0.0
        for (p in resampled) {
            if (p.x < minX) minX = p.x
            if (p.x > maxX) maxX = p.x
            if (p.y < minY) minY = p.y
            if (p.y > maxY) maxY = p.y
            sumX += p.x
            sumY += p.y
        }
        val w = maxX - minX
        val h = maxY - minY
        if (max(w, h) < 1e-9) return null
        val cx = sumX / resampled.size
        val cy = sumY / resampled.size
        val scale = SIZE / max(w, h)
        return resampled.map { Pt((it.x - cx) * scale, (it.y - cy) * scale) }
    }

    // ------------------------------------------------------------------
    // 内部
    // ------------------------------------------------------------------

    private data class Align(val theta: Double, val distance: Double)

    /**
     * 在「循环移位 × 正反笔顺」里找最小点对距离，同时给出让两者重合的最优旋转角。
     *
     * 旋转角有闭式解（最小二乘）：让 Σ|R(θ)·c - t|² 最小 ⇔ 让 Σ(R(θ)c)·t 最大
     * ⇔ θ = atan2(Σ(cx·ty - cy·tx), Σ(cx·tx + cy·ty))——不用黄金分割迭代，快且精确。
     */
    private fun bestAlign(cand: List<Pt>, tmpl: List<Pt>, allowShift: Boolean): Align? {
        var best = Double.MAX_VALUE
        var bestTheta = 0.0
        var bestTurn = Double.MAX_VALUE
        for (reverse in 0..1) {
            val seq = if (reverse == 1) cand.asReversed() else cand
            val shifts = if (allowShift) cand.indices else 0..0
            for (shift in shifts) {
                var a = 0.0
                var b = 0.0
                for (i in cand.indices) {
                    val c = seq[(i + shift) % cand.size]
                    val t = tmpl[i]
                    a += c.x * t.x + c.y * t.y
                    b += c.x * t.y - c.y * t.x
                }
                val theta = atan2(b, a)
                val cosine = cos(theta)
                val sine = sin(theta)
                var sum = 0.0
                for (i in cand.indices) {
                    val c = seq[(i + shift) % cand.size]
                    val t = tmpl[i]
                    val x = c.x * cosine - c.y * sine
                    val y = c.x * sine + c.y * cosine
                    sum += hypot(x - t.x, y - t.y)
                }
                val distance = sum / cand.size
                val turn = abs(degrees(theta))
                // 形状有 180° 旋转对称时（Z / N / S），反着画和正着画都能对上：
                // 距离几乎一样就**取转角小的那个**——否则「正着画 Z」会被判成 180°。
                val clearlyBetter = distance < best * (1.0 - TIE)
                val tieButStraighter = distance <= best * (1.0 + TIE) && turn < bestTurn - 1e-6
                if (clearlyBetter || tieButStraighter) {
                    best = distance
                    bestTheta = theta
                    bestTurn = turn
                }
            }
        }
        return if (best == Double.MAX_VALUE) null else Align(bestTheta, best)
    }

    /** 等弧长重采样到 [n] 点；点太少 / 长度为零返回 null。 */
    private fun resample(points: List<Pt>, n: Int): List<Pt>? {
        val clean = ArrayList<Pt>(points.size)
        for (p in points) {
            val prev = clean.lastOrNull()
            if (prev == null || hypot(p.x - prev.x, p.y - prev.y) > 1e-9) clean.add(p)
        }
        if (clean.size < 2) return null
        var total = 0.0
        for (i in 1 until clean.size) {
            total += hypot(clean[i].x - clean[i - 1].x, clean[i].y - clean[i - 1].y)
        }
        if (total < 1e-9) return null
        val interval = total / (n - 1)
        val out = ArrayList<Pt>(n)
        out.add(clean.first())
        var walked = 0.0
        var prev = clean.first()
        var i = 1
        while (out.size < n && i < clean.size) {
            val cur = clean[i]
            val seg = hypot(cur.x - prev.x, cur.y - prev.y)
            if (seg <= 1e-12) {
                prev = cur
                i++
                continue
            }
            if (walked + seg >= interval) {
                val k = (interval - walked) / seg
                val p = Pt(prev.x + k * (cur.x - prev.x), prev.y + k * (cur.y - prev.y))
                out.add(p)
                prev = p
                walked = 0.0
            } else {
                walked += seg
                prev = cur
                i++
            }
        }
        while (out.size < n) out.add(clean.last())
        return out
    }

    /** 角度归一到 (-180, 180]。 */
    private fun degrees(rad: Double): Double {
        var d = Math.toDegrees(rad) % 360.0
        if (d > 180.0) d -= 360.0
        if (d <= -180.0) d += 360.0
        return d
    }

    private fun bucketOf(deg: Double): Int = (((deg / 90.0).roundToInt() % 4) + 4) % 4

    private fun residualOf(deg: Double, bucket: Int): Double {
        var d = deg - bucket * 90.0
        while (d > 180.0) d -= 360.0
        while (d <= -180.0) d += 360.0
        return d
    }
}
