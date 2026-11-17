package com.kada.spike

import android.content.Context
import android.content.SharedPreferences
import android.util.Log
import org.json.JSONArray
import org.json.JSONObject
import kotlin.math.cos
import kotlin.math.hypot
import kotlin.math.roundToInt
import kotlin.math.sin
import kotlin.random.Random

/**
 * 画符号（S2b）：把「画一个符号」变成快捷键触发源。
 *
 * 手机上真正的痛点不是「键位不够」，是**没有键**。悬浮球面板只能「点一下」，
 * 条目一多就要滑；画一个符号是单手指能做到、又能一眼记住的入口。
 *
 * 三层分工：
 * 1. 识别在 [DollarOne]（纯计算，不认识任何业务）；
 * 2. 这一层负责「形状 + 方向 → 符号」「符号 → 绑哪条快捷键」「执行」，以及自检；
 * 3. 界面（[DrawPadView] / [DrawBoardView]）只收手指轨迹，执行走
 *    `KeyFilterService.runShortcut`——和面板、音量键**同一条动作链**。
 *
 * 绑定与自定义画法先存本机（`SharedPreferences("kada-draw")`），不进 `Config`：
 * 进配置要动 core 的模型与卡带格式，先把「画完能执行」「教它认自己的符号」跑通，再迁。
 */
object DrawStore {

    /** 一个可绑定的符号：预设是 `cls` + `bucket` 唯一确定，自定义符号则自己就是一类。 */
    data class Symbol(
        val id: String,
        val label: String,
        val glyph: String,
        val cls: String,
        val bucket: Int,
    )

    /**
     * 用户自己录的符号（P2）：模板就是他画的那一笔，**不判方向**。
     *
     * 为什么存原始笔画而不是归一化后的 32 点：归一化算法以后可能调（点数 / 缩放口径），
     * 存原始点意味着老的自定义符号能跟着新算法重新归一化；代价是文件大一点（一笔最多几百点）。
     */
    data class Custom(
        val id: String,
        val label: String,
        val points: List<DollarOne.Pt>,
        val closed: Boolean,
    ) {
        val glyph: String get() = label.trim().take(1).ifBlank { "✎" }

        fun toSymbol(): Symbol = Symbol(id, label, glyph, id, 0)
    }

    data class Outcome(
        val symbol: Symbol?,
        val reason: String,
        val score: Double,
        val runnerUp: String = "",
        val runOk: Boolean = false,
    )

    private const val PREFS = "kada-draw"
    private const val KEY_BINDINGS = "bindings"
    private const val KEY_CUSTOM = "custom"

    /** 自定义符号上限：再多就该用面板了，也防止有人误操作把识别拖慢到一帧一次重算。 */
    private const val MAX_CUSTOM = 24

    /** 自定义符号的 id 前缀：与 13 个预设类别名（circle / triangle / …）天然不撞。 */
    private const val CUSTOM_PREFIX = "u:"

    /** 低于这个分数就不执行（0.80 ≈ 平均点距 28/200，画歪一点仍算数）。 */
    private const val MIN_SCORE = 0.80

    /** 前两名差距小于这个值就判「太像」，宁可不执行。 */
    private const val MIN_MARGIN = 0.04

    /** 方向容差：扇区中心 ±35°，45° 附近一定落在两扇区之间 → 拒绝。 */
    private const val TOLERANCE_DEG = 35.0

    // ------------------------------------------------------------------
    // 预设符号表
    // ------------------------------------------------------------------

    val symbols: List<Symbol> = listOf(
        Symbol("circle", "圆", "○", "circle", 0),
        Symbol("diamond", "菱形", "◇", "diamond", 0),
        Symbol("star", "五角星", "☆", "star", 0),
        Symbol("check", "勾", "✓", "check", 0),
        Symbol("triangle", "三角", "△", "triangle", 0),
        Symbol("triangle_left", "左三角", "◁", "triangle", 1),
        Symbol("triangle_down", "倒三角", "▽", "triangle", 2),
        Symbol("triangle_right", "右三角", "▷", "triangle", 3),
        Symbol("vee", "字母 V", "V", "vee", 0),
        Symbol("zed", "字母 Z", "Z", "zed", 0),
        Symbol("ess", "字母 S", "S", "ess", 0),
        Symbol("ell", "字母 L", "L", "ell", 0),
        Symbol("see", "字母 C", "C", "see", 0),
        Symbol("you", "字母 U", "U", "you", 0),
        Symbol("em", "字母 M", "M", "em", 0),
        Symbol("dub", "字母 W", "W", "em", 2),
        Symbol("en", "字母 N", "N", "en", 0),
    )

    val symbolsById: Map<String, Symbol> = symbols.associateBy { it.id }

    /**
     * 形状类别与「方向是否有意义」。
     *
     * 圆 / 菱形 / 五角星 / 勾是**旋转对称或方向无意义**的：最优旋转角不稳定（圆的 θ
     * 可以落在任何地方），所以不能拿它判方向，也不能因为 45° 边界拒掉。
     * 三角与字母都是方向敏感的：三角的四个方向靠 θ 的四个扇区区分；
     * M/W 是同一形状转 180°，Z/N、U/C 靠「θ 不落在本扇区就淘汰」自然分开。
     */
    private val orientationFree = setOf("circle", "diamond", "star", "check")

    /** 闭合回路：允许「换个起笔点」（候选点序列循环移位）。开放折线移位后是另一条乱线。 */
    private val closedClasses = setOf("circle", "diamond", "star", "triangle")

    // ------------------------------------------------------------------
    // 模板（每个类别一份，方向靠 θ 判，不复制四份模板）
    // ------------------------------------------------------------------

    private val canonicalPaths: Map<String, List<DollarOne.Pt>> = linkedMapOf(
        "circle" to circlePath(),
        "diamond" to path(0.5, 0.05, 0.95, 0.5, 0.5, 0.95, 0.05, 0.5, 0.5, 0.05),
        "star" to starPath(),
        "check" to path(0.08, 0.55, 0.35, 0.88, 0.92, 0.12),
        "triangle" to path(0.5, 0.05, 0.95, 0.95, 0.05, 0.95, 0.5, 0.05),
        "vee" to path(0.08, 0.08, 0.5, 0.92, 0.92, 0.08),
        "zed" to path(0.08, 0.08, 0.92, 0.08, 0.08, 0.92, 0.92, 0.92),
        "ess" to path(
            0.88, 0.18, 0.62, 0.05, 0.28, 0.14, 0.16, 0.36, 0.30, 0.52,
            0.70, 0.50, 0.84, 0.70, 0.66, 0.92, 0.32, 0.95, 0.12, 0.82,
        ),
        "ell" to path(0.10, 0.08, 0.10, 0.92, 0.90, 0.92),
        "see" to ceePath(),
        "you" to path(0.10, 0.08, 0.10, 0.66, 0.20, 0.90, 0.50, 0.96, 0.80, 0.90, 0.90, 0.66, 0.90, 0.08),
        "em" to path(0.08, 0.92, 0.08, 0.08, 0.5, 0.58, 0.92, 0.08, 0.92, 0.92),
        "en" to path(0.10, 0.92, 0.10, 0.08, 0.90, 0.92, 0.90, 0.08),
    )

    private val presetTemplates: List<DollarOne.Template> by lazy {
        canonicalPaths.mapNotNull { (cls, pts) ->
            DollarOne.template(
                cls = cls,
                directional = cls !in orientationFree,
                path = pts,
                closed = cls in closedClasses,
            )
        }
    }

    /** 自定义符号缓存：录 / 删 / 临时自检符号都会让它整份失效。 */
    private var customCache: List<Custom>? = null
    private var templateCache: List<DollarOne.Template>? = null

    /** 自检用的临时符号：不落盘，跑完立刻清掉。 */
    @Volatile
    private var ephemeral: List<Custom> = emptyList()

    @Synchronized
    fun custom(): List<Custom> {
        customCache?.let { return it }
        val loaded = loadCustom()
        customCache = loaded
        return loaded
    }

    /** 预设模板 + 自定义模板；自定义一律 `directional = false`（θ 不稳定，也不该判方向）。 */
    @Synchronized
    private fun allTemplates(): List<DollarOne.Template> {
        templateCache?.let { return it }
        val customTemplates = custom().mapNotNull { c ->
            DollarOne.template(c.id, directional = false, path = c.points, closed = c.closed)
        }
        val all = presetTemplates + customTemplates
        templateCache = all
        return all
    }

    @Synchronized
    private fun invalidate() {
        customCache = null
        templateCache = null
    }

    // ------------------------------------------------------------------
    // 对外状态（报告 / adb 断言读）
    // ------------------------------------------------------------------

    @Volatile
    var lastSymbol: String = ""
        private set

    @Volatile
    var lastScore: Double = 0.0
        private set

    @Volatile
    var lastRun: String = ""
        private set

    @Volatile
    var lastError: String = ""
        private set

    @Volatile
    private var lastSelfTest: JSONObject? = null

    fun lastSelfTestJson(): JSONObject? = lastSelfTest

    // ------------------------------------------------------------------
    // 绑定
    // ------------------------------------------------------------------

    fun bindings(): LinkedHashMap<String, String> {
        val raw = prefs()?.getString(KEY_BINDINGS, "")?.takeIf { it.isNotBlank() } ?: return LinkedHashMap()
        return runCatching {
            val obj = JSONObject(raw)
            val out = LinkedHashMap<String, String>()
            for (key in obj.keys()) {
                if (symbolById(key) == null) continue
                val value = obj.optString(key).trim()
                if (value.isNotEmpty()) out[key] = value
            }
            out
        }.getOrDefault(LinkedHashMap())
    }

    fun bind(symbolId: String, shortcutName: String): Boolean {
        val symbol = symbolById(symbolId) ?: return fail("没有这个符号：$symbolId")
        val name = shortcutName.trim()
        if (name.isEmpty()) return unbind(symbolId)
        if (name !in ShortcutStore.entries().filter { it.enabled }.map { it.name }) {
            return fail("配置里没有启用中的快捷键「$name」")
        }
        val map = bindings()
        map[symbolId] = name
        persist(map)
        lastError = ""
        Log.i(SpikeState.TAG, "draw bind ${symbol.label} -> $name")
        return true
    }

    fun unbind(symbolId: String): Boolean {
        val map = bindings()
        if (map.remove(symbolId) == null) return true
        persist(map)
        lastError = ""
        Log.i(SpikeState.TAG, "draw unbind $symbolId")
        return true
    }

    fun clearBindings() {
        persist(LinkedHashMap())
        lastError = ""
    }

    // ------------------------------------------------------------------
    // 自定义画法（P2）：用户教它认自己的符号
    // ------------------------------------------------------------------

    /** 预设 + 自定义，界面按这个顺序列表（预设在前，自定义在后）。 */
    fun allSymbols(): List<Symbol> = symbols + custom().map { it.toSymbol() }

    fun symbolById(id: String): Symbol? = symbolsById[id] ?: custom().firstOrNull { it.id == id }?.toSymbol()

    /** 记住一条笔画。返回新符号的 id；失败返回空串并写 [lastError]。 */
    fun learn(points: List<DollarOne.Pt>, label: String): String {
        val clean = points.filter { it.x.isFinite() && it.y.isFinite() }
        if (clean.size < 2) return learnFail("笔画太短：至少画一条线段")
        if (DollarOne.normalize(clean) == null) return learnFail("笔画太短：至少画一条线段")
        val existing = custom()
        if (existing.size >= MAX_CUSTOM) return learnFail("自定义符号最多 $MAX_CUSTOM 个，先删几个")

        // 与已有符号「几乎一样」就别收了：两份都留着只会让每次识别都在边界上抖。
        val result = DollarOne.recognize(clean, allTemplates(), TOLERANCE_DEG)
        val best = result?.ranked?.firstOrNull()
        if (best != null && best.score >= 0.90) {
            return learnFail("和「${labelOfClass(best.cls)}」几乎一样（${fmt(best.score)}），换个画法")
        }

        val id = CUSTOM_PREFIX + System.currentTimeMillis().toString(36)
        val name = label.trim().ifEmpty { "自定义 ${existing.size + 1}" }.take(12)
        val first = clean.first()
        val last = clean.last()
        val total = clean.zipWithNext().sumOf { (a, b) -> hypot(b.x - a.x, b.y - a.y) }
        val closed = total > 1e-9 && hypot(last.x - first.x, last.y - first.y) <= total * 0.15

        val list = existing + Custom(id, name, clean.map { DollarOne.Pt(round4(it.x), round4(it.y)) }, closed)
        persistCustom(list)
        invalidate()
        lastError = ""
        Log.i(SpikeState.TAG, "draw learn $id ${name} closed=$closed points=${clean.size}")
        return id
    }

    /** 删掉一个自定义符号（绑定一起清掉，否则会留下取不出来的键）。 */
    fun forget(symbolId: String): Boolean {
        if (!symbolId.startsWith(CUSTOM_PREFIX)) return fail("只能删自定义符号：$symbolId")
        val list = custom()
        if (list.none { it.id == symbolId }) return fail("没有这个自定义符号：$symbolId")
        unbind(symbolId)
        persistCustom(list.filter { it.id != symbolId })
        invalidate()
        lastError = ""
        Log.i(SpikeState.TAG, "draw forget $symbolId")
        return true
    }

    private fun loadCustom(): List<Custom> {
        val raw = prefs()?.getString(KEY_CUSTOM, "")?.takeIf { it.isNotBlank() } ?: return ephemeral
        val parsed = runCatching {
            val arr = JSONArray(raw)
            val out = ArrayList<Custom>(arr.length())
            for (i in 0 until arr.length()) {
                val obj = arr.optJSONObject(i) ?: continue
                val id = obj.optString("id").trim().takeIf { it.startsWith(CUSTOM_PREFIX) } ?: continue
                val pts = ArrayList<DollarOne.Pt>()
                val arrPts = obj.optJSONArray("points") ?: continue
                for (j in 0 until arrPts.length()) {
                    val pair = arrPts.optJSONArray(j) ?: continue
                    if (pair.length() != 2) continue
                    pts.add(DollarOne.Pt(pair.optDouble(0), pair.optDouble(1)))
                }
                if (pts.size < 2) continue
                out.add(
                    Custom(
                        id = id,
                        label = obj.optString("label").ifBlank { "自定义" }.take(12),
                        points = pts,
                        closed = obj.optBoolean("closed", false),
                    ),
                )
            }
            out
        }.getOrDefault(ArrayList())
        return parsed + ephemeral
    }

    private fun persistCustom(list: List<Custom>) {
        val arr = JSONArray()
        for (c in list) {
            val pts = JSONArray()
            for (p in c.points) {
                pts.put(JSONArray().put(round4(p.x)).put(round4(p.y)))
            }
            arr.put(
                JSONObject().apply {
                    put("id", c.id)
                    put("label", c.label)
                    put("closed", c.closed)
                    put("points", pts)
                },
            )
        }
        prefs()?.edit()?.putString(KEY_CUSTOM, arr.toString())?.apply()
    }

    private fun learnFail(message: String): String {
        lastError = message
        Log.i(SpikeState.TAG, "draw learn failed: $message")
        return ""
    }

    private fun round4(value: Double): Double = Math.round(value * 10000.0) / 10000.0

    // ------------------------------------------------------------------
    // 识别 / 执行
    // ------------------------------------------------------------------

    fun recognize(points: List<DollarOne.Pt>): Outcome {
        val result = DollarOne.recognize(points, allTemplates(), TOLERANCE_DEG)
            ?: return Outcome(null, "笔画太短：至少画一条线段", 0.0)

        // 只让「这个方向真的有预设符号」的候选参与竞争：Z 转 90° 很像 N，但 N 只有 0°
        // 这一档——转 90° 的 N 不该把 Z 挤下去（每类只有一个符号时这条尤其重要）。
        val candidates = result.ranked.filter { symbolFor(it.cls, it.bucket) != null }
        val edge = result.edgeRejected.firstOrNull { symbolFor(it.cls, it.bucket) != null }
        val best = candidates.firstOrNull()
        if (best == null) {
            return if (edge != null) {
                Outcome(
                    null,
                    "方向不明确（约 ${edge.thetaDeg.roundToInt()}°，像两个符号之间），重画",
                    edge.score,
                    labelOfClass(edge.cls),
                )
            } else {
                Outcome(null, "没认出来", result.score)
            }
        }

        lastScore = best.score
        if (edge != null && edge.score > best.score + MIN_MARGIN) {
            return Outcome(
                null,
                "方向不明确（约 ${edge.thetaDeg.roundToInt()}°，像两个符号之间），重画",
                edge.score,
                labelOfClass(edge.cls),
            )
        }
        if (best.score < MIN_SCORE) {
            return Outcome(null, "没认出来（最像「${labelOfClass(best.cls)}」${fmt(best.score)}）", best.score)
        }
        val second = candidates.getOrNull(1)
        if (second != null && best.score - second.score < MIN_MARGIN) {
            return Outcome(
                null,
                "和「${labelOfClass(second.cls)}」太像了，重画一次",
                best.score,
                labelOfClass(second.cls),
            )
        }
        val symbol = symbolFor(best.cls, best.bucket)
            ?: return Outcome(null, "认成「${labelOfClass(best.cls)}·${best.bucket * 90}°」——这个方向没有预设符号", best.score)
        return Outcome(symbol, "", best.score)
    }

    /** 识别 + 执行一条绑定；返回的 [Outcome.symbol] 非空即代表认出来了（执行结果看 [Outcome.runOk]）。 */
    fun runPath(points: List<DollarOne.Pt>, source: String = "draw"): Outcome {
        val outcome = recognize(points)
        val symbol = outcome.symbol
        if (symbol == null) {
            lastSymbol = ""
            lastError = outcome.reason
            Log.i(SpikeState.TAG, "draw miss ${outcome.reason}")
            return outcome
        }
        lastSymbol = symbol.id
        lastScore = outcome.score
        val ok = runSymbol(symbol.id, source)
        return outcome.copy(runOk = ok)
    }

    /** 执行某个符号绑定的快捷键。返回 false 时 [lastError] 一定有话说。 */
    fun runSymbol(symbolId: String, source: String = "draw"): Boolean {
        val symbol = symbolById(symbolId) ?: return fail("没有这个符号：$symbolId")
        val name = bindings()[symbolId]
        if (name.isNullOrBlank()) return fail("「${symbol.label}」还没绑定快捷键")
        val entry = ShortcutStore.entries().firstOrNull { it.name == name && it.enabled }
            ?: return fail("绑定的「$name」不在启用中的快捷键里（改名 / 删除 / 停用了？）")
        val service = KeyFilterService.instance ?: return fail("无障碍服务未连接，没执行")
        if (!service.runShortcut(entry.index, source)) return fail("执行失败：这条快捷键没有动作")
        lastError = ""
        lastRun = "$source:${symbol.label}→${entry.name}"
        Log.i(SpikeState.TAG, "draw run $lastRun")
        return true
    }

    /** adb 用：`0.1,0.2;0.5,0.9`（0~1 归一化坐标）→ 点序列。 */
    fun parsePath(raw: String): List<DollarOne.Pt> {
        val out = ArrayList<DollarOne.Pt>()
        for (chunk in raw.split(';')) {
            if (chunk.isBlank()) continue
            val xy = chunk.split(',')
            if (xy.size != 2) continue
            val x = xy[0].trim().toDoubleOrNull() ?: continue
            val y = xy[1].trim().toDoubleOrNull() ?: continue
            out.add(DollarOne.Pt(x, y))
        }
        return out
    }

    // ------------------------------------------------------------------
    // 自检（识别器是纯计算，断言都在这里跑；`verify.ps1` 读报告）
    // ------------------------------------------------------------------

    /**
     * 识别器自检：预设符号、方向、45° 边界、起点 / 笔顺、乱涂。
     *
     * 为什么不用 JVM 单测：Gradle 离线时加测试依赖会失败，而这里断言的东西
     * （旋转方向、阈值）恰恰只有真跑一遍才作数；跑在设备上还能顺带验打包后的 `.so` 之外的代码。
     */
    fun selfTest(): JSONObject {
        val cases = JSONArray()
        var failed = 0

        fun add(name: String, want: String?, points: List<DollarOne.Pt>) {
            val outcome = recognize(points)
            val got = outcome.symbol?.id.orEmpty()
            val expect = want.orEmpty()
            val ok = got == expect
            if (!ok) failed++
            val item = JSONObject().apply {
                put("name", name)
                put("want", expect)
                put("got", got)
                put("ok", ok)
                put("score", round3(outcome.score))
                put("reason", outcome.reason)
            }
            if (!ok) {
                // 失败时把前几名连同 θ / 扇区一起写进报告：真机上调阈值全靠这几行
                val raw = DollarOne.recognize(points, allTemplates(), TOLERANCE_DEG)
                if (raw != null) {
                    val top = (raw.ranked + raw.edgeRejected).sortedByDescending { it.score }.take(4)
                    val rank = JSONArray()
                    for (s in top) {
                        rank.put(
                            JSONObject().apply {
                                put("cls", s.cls)
                                put("score", round3(s.score))
                                put("theta", s.thetaDeg.roundToInt())
                                put("bucket", s.bucket)
                                put("residual", s.residualDeg.roundToInt())
                                put("valid", s.valid)
                                put("hasSymbol", symbolFor(s.cls, s.bucket) != null)
                            },
                        )
                    }
                    item.put("rank", rank)
                }
            }
            cases.put(item)
        }

        // 1) 每个预设符号：把模板转到它自己的方向，加 1.2% 抖动 —— 必须认回自己
        symbols.forEachIndexed { index, symbol ->
            val base = canonicalPaths[symbol.cls] ?: emptyList()
            val drawn = rotatePath(base, -90.0 * symbol.bucket)
            add("preset:${symbol.id}", symbol.id, jitter(drawn, 0.012, index + 1))
        }

        // 2) 歪 ±20° 仍算同一个方向
        add("tilt:+20", "triangle", rotatePath(canonicalPaths.getValue("triangle"), 20.0))
        add("tilt:-20", "triangle", rotatePath(canonicalPaths.getValue("triangle"), -20.0))

        // 3) 45° 左右不猜方向（宁可没反应）
        add("edge:44", null, rotatePath(canonicalPaths.getValue("triangle"), 44.0))

        // 4) 四个方向用「手写坐标」独立构造：不复用 rotatePath，免得把符号约定错在同一个地方
        add("geo:up", "triangle", path(0.50, 0.06, 0.94, 0.94, 0.06, 0.94, 0.50, 0.06))
        add("geo:down", "triangle_down", path(0.50, 0.94, 0.06, 0.06, 0.94, 0.06, 0.50, 0.94))
        add("geo:left", "triangle_left", path(0.06, 0.50, 0.94, 0.06, 0.94, 0.94, 0.06, 0.50))
        add("geo:right", "triangle_right", path(0.94, 0.50, 0.06, 0.06, 0.06, 0.94, 0.94, 0.50))

        // 5) 换个起笔点 / 反着画，圆还是圆（原版 $1 这两条都会不命中）
        val circle = canonicalPaths.getValue("circle")
        add("start:circle", "circle", circle.drop(7) + circle.take(7))
        add("reverse:circle", "circle", circle.reversed())

        // 6) 乱涂 / 太短 → 什么都不执行
        add("scribble", null, path(0.10, 0.20, 0.80, 0.10, 0.20, 0.70, 0.90, 0.60, 0.15, 0.90, 0.60, 0.35, 0.85, 0.85, 0.30, 0.45))
        add("too-short", null, path(0.5, 0.5, 0.5, 0.5, 0.5005, 0.5))

        // 7) 自定义画法：走与「用户录一个符号」同一条路（模板从 Custom 建，只是不落盘）。
        //    形状选「楼梯」——13 个预设里没有，避免这条用例被预设抢走而失去意义。
        val stairs = path(0.10, 0.85, 0.10, 0.50, 0.45, 0.50, 0.45, 0.15, 0.90, 0.15)
        ephemeral = listOf(Custom("$CUSTOM_PREFIX selftest", "自检楼梯", stairs, closed = false))
        invalidate()
        try {
            add("custom:hit", "$CUSTOM_PREFIX selftest", jitter(stairs, 0.012, 77))
            // 自定义符号不判方向：转 30° 仍该认出来（预设的三角在这里会被 45° 边界拒掉）
            add("custom:tilt", "$CUSTOM_PREFIX selftest", rotatePath(stairs, 30.0))
            add("custom:miss", null, path(0.10, 0.20, 0.80, 0.10, 0.20, 0.70, 0.90, 0.60))
        } finally {
            ephemeral = emptyList()
            invalidate()
        }

        val json = JSONObject().apply {
            put("total", cases.length())
            put("failed", failed)
            put("templates", presetTemplates.size)
            put("symbols", symbols.size)
            put("custom", custom().size)
            put("cases", cases)
            put("generatedAt", System.currentTimeMillis())
        }
        lastSelfTest = json
        Log.i(SpikeState.TAG, "draw selfTest total=${cases.length()} failed=$failed preset=${presetTemplates.size}")
        return json
    }

    fun reportJson(): JSONObject = JSONObject().apply {
        put("symbols", symbols.size)
        put("templates", presetTemplates.size)
        put("custom", custom().size)
        put("customIds", JSONArray(custom().map { it.id }))
        put("bindings", JSONObject(bindings() as Map<*, *>))
        put("lastSymbol", lastSymbol)
        put("lastScore", round3(lastScore))
        put("lastRun", lastRun)
        put("lastError", lastError)
        lastSelfTest?.let { put("selfTest", it) }
    }

    // ------------------------------------------------------------------
    // 内部
    // ------------------------------------------------------------------

    private fun prefs(): SharedPreferences? =
        SpikeState.appContext?.getSharedPreferences(PREFS, Context.MODE_PRIVATE)

    private fun persist(map: Map<String, String>) {
        val editor = prefs()?.edit() ?: return
        val obj = JSONObject()
        for ((key, value) in map) obj.put(key, value)
        editor.putString(KEY_BINDINGS, obj.toString()).apply()
    }

    private fun fail(message: String): Boolean {
        lastError = message
        Log.i(SpikeState.TAG, "draw $message")
        return false
    }

    private fun labelOfClass(cls: String): String =
        symbols.firstOrNull { it.cls == cls }?.label
            ?: custom().firstOrNull { it.id == cls }?.label
            ?: cls

    private fun symbolFor(cls: String, bucket: Int): Symbol? =
        symbols.firstOrNull { it.cls == cls && it.bucket == bucket }
            ?: custom().firstOrNull { it.id == cls }?.toSymbol()

    private fun fmt(value: Double): String = String.format(java.util.Locale.US, "%.2f", value)

    private fun round3(value: Double): Double = Math.round(value * 1000.0) / 1000.0

    private fun path(vararg xy: Double): List<DollarOne.Pt> =
        (0 until xy.size / 2).map { DollarOne.Pt(xy[it * 2], xy[it * 2 + 1]) }

    private fun circlePath(): List<DollarOne.Pt> {
        val pts = (0 until 24).map {
            val t = 2.0 * Math.PI * it / 24.0
            DollarOne.Pt(0.5 + 0.45 * cos(t), 0.5 + 0.45 * sin(t))
        }
        return pts + pts.first()
    }

    private fun starPath(): List<DollarOne.Pt> {
        val pts = (0 until 10).map {
            val r = if (it % 2 == 0) 0.47 else 0.19
            val t = -Math.PI / 2 + it * Math.PI / 5.0
            DollarOne.Pt(0.5 + r * cos(t), 0.5 + r * sin(t))
        }
        return pts + pts.first()
    }

    /** 「C」：从右下绕过左侧画到右上，开口朝右（弧长约 220°，画满一圈就成圆了）。 */
    private fun ceePath(): List<DollarOne.Pt> = (0..10).map {
        val t = Math.toRadians(70.0 + it * 22.0)
        DollarOne.Pt(0.5 + 0.44 * cos(t), 0.5 + 0.44 * sin(t))
    }

    /** 绕原点旋转（自检用）：与识别器内部的旋转矩阵同一套公式。 */
    private fun rotatePath(points: List<DollarOne.Pt>, deg: Double): List<DollarOne.Pt> {
        val rad = Math.toRadians(deg)
        val c = cos(rad)
        val s = sin(rad)
        return points.map { DollarOne.Pt(it.x * c - it.y * s, it.x * s + it.y * c) }
    }

    /** 确定性抖动：同一个 seed 每次跑出同一组噪声，自检结果可复现。 */
    private fun jitter(points: List<DollarOne.Pt>, amount: Double, seed: Int): List<DollarOne.Pt> {
        val rnd = Random(seed)
        return points.map {
            DollarOne.Pt(
                it.x + (rnd.nextDouble() - 0.5) * amount,
                it.y + (rnd.nextDouble() - 0.5) * amount,
            )
        }
    }
}
