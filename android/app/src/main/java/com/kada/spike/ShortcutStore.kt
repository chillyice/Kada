package com.kada.spike

import org.json.JSONArray
import org.json.JSONObject

/**
 * 编辑器数据层（S1-d）：把一条快捷键读成界面看得懂的字段，改完再整份交给 Rust 落盘。
 *
 * **边界**：这里只搬 JSON 文档，不解析触发键、不判支持不支持——解析与撞车判定在
 * `RustBridge.checkTrigger`（core 的 `Trigger::parse_loose`），清洗与原子写在
 * `RustBridge.save`（`kada_core::sanitize_config` + `config_io::save`）。
 *
 * **动作链只编编辑器认得的那几类**（打开网址 / 文本 / 剪贴板 / 语义动作 / 等待）：
 * 编辑别的类型时用 `ACTION_KEEP`，原动作链一个字节都不动——手机屏上做不了桌面的全量编辑器，
 * 但也不许「改个触发键把动作改没了」。
 */
object ShortcutStore {

    /** 一条快捷键的编辑器视图。 */
    data class Entry(
        val index: Int,
        val name: String,
        val triggers: List<String>,
        val enabled: Boolean,
        val actionType: String,
        val actionValue: String,
        val actionSummary: String,
        val layer: String?,
        val actionCount: Int,
    )

    /** 「保留原动作链」：编辑时不动 `actions`（新建时也用它表示「先不填动作」）。 */
    const val ACTION_KEEP = "keep"

    /** 编辑器能新建的动作类型（顺序即界面顺序）。 */
    val actionTypes: List<String> = listOf(
        ACTION_KEEP,
        "open_url",
        "text",
        "clipboard",
        "semantic",
        "pause_ms",
    )

    data class Check(
        val ok: Boolean,
        val canonical: String,
        val kind: String,
        val error: String,
        val normalized: Boolean,
        /** 「撞车」的其他条目描述（`#0 名称`），空 = 没人占用。 */
        val duplicates: List<String>,
    )

    data class Save(
        val ok: Boolean,
        val error: String,
        /** Rust 清洗时被忽略的条目说明（坏触发键 / 空动作……）。 */
        val ignored: List<String>,
        /** 保存后配置里的硬冲突条数（重复触发键等；core 判硬冲突）。 */
        val hardConflicts: Int,
        val shortcuts: Int,
    )

    /** 最近一次编辑动作的结果（报告导出 / adb 断言用）。 */
    @Volatile
    var lastResult: String = ""
        private set

    /** 最近一次触发键校验结果（报告导出 / adb 断言用）。 */
    @Volatile
    var lastCheck: Check? = null
        private set

    fun entries(): List<Entry> {
        val cfg = config() ?: return emptyList()
        val arr = cfg.optJSONArray("shortcuts") ?: return emptyList()
        return (0 until arr.length()).mapNotNull { i ->
            arr.optJSONObject(i)?.let { entry(i, it) }
        }
    }

    /** 手机档面板执行用：取某条目的原始动作链（不解释，交给 `ActionRunner`）。 */
    fun actionsAt(index: Int): JSONArray? {
        val arr = config()?.optJSONArray("shortcuts") ?: return null
        return arr.optJSONObject(index)?.optJSONArray("actions")
    }

    fun check(trigger: String, excludeIndex: Int): Check {
        val obj = RustBridge.checkTrigger(trigger, excludeIndex)
        val check = if (obj == null) {
            Check(false, trigger, "", "Rust 未加载（libkada_android.so 缺失？）", false, emptyList())
        } else {
            Check(
                ok = obj.optBoolean("ok", false),
                canonical = obj.optString("canonical").ifEmpty { trigger },
                kind = obj.optString("kind"),
                error = obj.optString("error"),
                normalized = obj.optBoolean("normalized", false),
                duplicates = obj.optJSONArray("duplicates")?.let { arr ->
                    (0 until arr.length()).map { i ->
                        val d = arr.optJSONObject(i) ?: JSONObject()
                        val name = d.optString("name").ifEmpty { "(未命名)" }
                        "#${d.optInt("item")} $name"
                    }
                } ?: emptyList(),
            )
        }
        lastCheck = check
        return check
    }

    /**
     * 新建（`index == null`）或保存某一条；`trigger` 只替换**第一条**触发键，
     * 同一条目上其余触发键原样保留（界面上也只给了一个触发键框）。
     *
     * 触发键在**这里**先过一遍 Rust 校验（大小写补正 + 撞车）：界面与 adb 都走这条路，
     * 否则 `f4` 这种小写写法会被 `sanitize_config` 当成坏触发键悄悄丢掉，留下一条「有名字没触发键」。
     */
    fun save(
        index: Int?,
        name: String,
        trigger: String,
        actionType: String,
        actionValue: String,
        enabled: Boolean,
    ): Save {
        val cfg = config() ?: return fail("Rust 未加载，配置读不出来")
        val arr = cfg.optJSONArray("shortcuts") ?: JSONArray()
        val isNew = index == null || index < 0 || index >= arr.length()
        val item = if (isNew) JSONObject() else arr.getJSONObject(index!!)

        if (trigger.isBlank()) return fail("触发键不能为空")
        val check = check(trigger, if (isNew) -1 else index!!)
        if (!check.ok) return fail("触发键无效：${check.error}")
        if (check.duplicates.isNotEmpty()) {
            return fail("触发键与 ${check.duplicates.joinToString("、")} 重复")
        }

        val triggers = JSONArray().apply {
            put(check.canonical)
            item.optJSONArray("triggers")?.let { old ->
                for (i in 1 until old.length()) put(old.optString(i))
            }
        }
        item.put("triggers", triggers)
        if (name.isBlank()) item.remove("name") else item.put("name", name.trim())
        item.put("enabled", enabled)

        when {
            actionType == ACTION_KEEP && isNew -> item.put("actions", JSONArray())
            actionType == ACTION_KEEP -> Unit
            else -> item.put("actions", JSONArray().put(buildAction(actionType, actionValue)))
        }
        if (isNew) arr.put(item)

        cfg.put("shortcuts", arr)
        return saveConfig(cfg, if (isNew) "新建" else "保存")
    }

    /** 删除一条（界面先弹确认；这里只负责落盘）。 */
    fun delete(index: Int): Save {
        val cfg = config() ?: return fail("Rust 未加载，配置读不出来")
        val arr = cfg.optJSONArray("shortcuts") ?: JSONArray()
        if (index < 0 || index >= arr.length()) return fail("条目下标越界：$index")
        val next = JSONArray()
        for (i in 0 until arr.length()) {
            if (i != index) next.put(arr.getJSONObject(i))
        }
        cfg.put("shortcuts", next)
        return saveConfig(cfg, "删除")
    }

    /** 动作类型的界面名（编辑器的下拉与列表摘要共用）。 */
    fun actionLabel(type: String): String = when (type) {
        ACTION_KEEP -> "保留原动作"
        "open_url" -> "打开网址"
        "text" -> "输入文本"
        "clipboard" -> "写入剪贴板"
        "semantic" -> "语义动作"
        "pause_ms" -> "等待（毫秒）"
        else -> type
    }

    // ------------------------------------------------------------------
    // 内部
    // ------------------------------------------------------------------

    private fun config(): JSONObject? = runCatching { JSONObject(RustBridge.exportText()) }.getOrNull()

    private fun entry(index: Int, obj: JSONObject): Entry {
        val actions = obj.optJSONArray("actions") ?: JSONArray()
        val first = actions.optJSONObject(0)
        val type = first?.optString("type").orEmpty()
        return Entry(
            index = index,
            name = obj.optString("name"),
            triggers = obj.optJSONArray("triggers").toStringList(),
            enabled = obj.optBoolean("enabled", false),
            actionType = if (type in actionTypes) type else ACTION_KEEP,
            actionValue = actionValue(first),
            actionSummary = summary(actions),
            layer = obj.optString("layer").ifEmpty { null },
            actionCount = actions.length(),
        )
    }

    /** 编辑框里预填的「动作参数」：按动作类型取那一个字段。 */
    private fun actionValue(action: JSONObject?): String {
        action ?: return ""
        return when (action.optString("type")) {
            "open_url" -> action.optString("url")
            "text" -> action.optString("text")
            "semantic" -> action.optString("operation")
            "clipboard" -> action.optJSONObject("operation")?.optString("text").orEmpty()
            "pause_ms" -> action.optLong("ms").toString()
            else -> ""
        }
    }

    private fun buildAction(type: String, value: String): JSONObject = when (type) {
        "open_url" -> JSONObject().put("type", "open_url").put("url", value.trim())
        "text" -> JSONObject().put("type", "text").put("text", value)
        "clipboard" -> JSONObject().put("type", "clipboard").put(
            "operation",
            JSONObject().put("op", "set_clipboard").put("text", value),
        )
        "semantic" -> JSONObject().put("type", "semantic").put("operation", value.trim())
        "pause_ms" -> JSONObject().put("type", "pause_ms").put(
            "ms",
            value.trim().toLongOrNull()?.coerceIn(0, 60_000) ?: 0,
        )
        else -> JSONObject().put("type", ACTION_KEEP)
    }

    private fun summary(actions: JSONArray): String {
        if (actions.length() == 0) return "（没有动作）"
        val parts = (0 until actions.length()).mapNotNull { i ->
            actions.optJSONObject(i)?.let { describe(it) }
        }
        return parts.joinToString(" → ")
    }

    private fun describe(action: JSONObject): String = when (val type = action.optString("type")) {
        "open_url" -> "打开网址 ${action.optString("url")}"
        "text" -> "输入文本「${clip(action.optString("text"))}」"
        "clipboard" -> when (action.optJSONObject("operation")?.optString("op")) {
            "set_clipboard" -> "写入剪贴板「${clip(action.optJSONObject("operation")?.optString("text").orEmpty())}」"
            "tempstr" -> "临时文本「${clip(action.optJSONObject("operation")?.optString("text").orEmpty())}」"
            "get_clipboard" -> "读剪贴板 → 变量"
            else -> "剪贴板动作"
        }
        "semantic" -> {
            val op = action.optString("operation")
            "语义动作：${ActionCatalog.byId(op)?.label ?: op}"
        }
        "pause_ms" -> "等待 ${action.optLong("ms")} ms"
        "group" -> "分组（${action.optJSONArray("actions")?.length() ?: 0} 步）"
        "disabled" -> "（已禁用，跳过）"
        else -> type.ifEmpty { "未知动作" }
    }

    private fun saveConfig(cfg: JSONObject, what: String): Save {
        val report = RustBridge.save(cfg.toString())
            ?: return fail("Rust 未加载，未保存")
        if (!report.optBoolean("ok", false)) {
            return fail(report.optString("error").ifEmpty { "保存失败" })
        }
        val ignored = report.optJSONArray("ignored").toStringList()
        var hard = 0
        report.optJSONArray("conflicts")?.let { arr ->
            for (i in 0 until arr.length()) {
                if (arr.optJSONObject(i)?.optString("severity") == "error") hard++
            }
        }
        val shortcuts = report.optJSONObject("counts")?.optInt("shortcuts", -1) ?: -1
        lastResult = buildString {
            append("$what：快捷键 $shortcuts 条")
            if (ignored.isNotEmpty()) append("，Rust 忽略 ${ignored.size} 处")
            if (hard > 0) append("，硬冲突 $hard 条")
        }
        return Save(true, "", ignored, hard, shortcuts)
    }

    private fun fail(message: String): Save {
        lastResult = "编辑失败：$message"
        return Save(false, message, emptyList(), 0, -1)
    }

    private fun clip(text: String, max: Int = 24): String =
        if (text.length <= max) text else text.take(max) + "…"

    private fun JSONArray?.toStringList(): List<String> {
        if (this == null) return emptyList()
        return (0 until length()).map { optString(it) }
    }
}
