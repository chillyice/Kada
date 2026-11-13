package com.kada.spike

import android.accessibilityservice.AccessibilityService
import android.os.Build
import android.os.Bundle
import android.view.accessibility.AccessibilityNodeInfo
import android.view.accessibility.AccessibilityWindowInfo
import android.widget.Toast

data class ActionResult(val ok: Boolean, val detail: String)

/**
 * 语义动作表（docs/安卓版规划.md §3.2）：无 root 档唯一能落地的「改键」替身。
 * `minSdk` 用来在界面上标「本机 API 够不够」，`needsInput` 表示必须先把光标放进输入框。
 */
data class ActionSpec(
    val id: String,
    val label: String,
    val api: String,
    val minSdk: Int,
    val needsInput: Boolean = false,
    val note: String = "",
)

object ActionCatalog {
    val specs: List<ActionSpec> = listOf(
        ActionSpec("copy", "复制", "ACTION_COPY", 26, needsInput = true),
        ActionSpec("paste", "粘贴", "ACTION_PASTE", 26, needsInput = true),
        ActionSpec("cut", "剪切", "ACTION_CUT", 26, needsInput = true),
        ActionSpec("selectAll", "全选", "ACTION_SET_SELECTION 0..n", 26, needsInput = true, note = "SDK 没有 ACTION_SELECT_ALL，用选区整个文本模拟"),
        ActionSpec("imeEnter", "提交", "ACTION_IME_ENTER", 30, needsInput = true),
        ActionSpec("setText", "写入文本", "ACTION_SET_TEXT", 26, needsInput = true, note = "文本落地实验"),
        ActionSpec("scrollBackward", "上翻页", "ACTION_SCROLL_BACKWARD", 26),
        ActionSpec("scrollForward", "下翻页", "ACTION_SCROLL_FORWARD", 26),
        ActionSpec("recents", "最近任务", "GLOBAL_ACTION_RECENTS", 26),
        ActionSpec("home", "主屏", "GLOBAL_ACTION_HOME", 26),
        ActionSpec("back", "返回", "GLOBAL_ACTION_BACK", 26),
        ActionSpec("notifications", "通知栏", "GLOBAL_ACTION_NOTIFICATIONS", 26),
        ActionSpec("quickSettings", "控制中心", "GLOBAL_ACTION_QUICK_SETTINGS", 26),
        ActionSpec("lockScreen", "锁屏", "GLOBAL_ACTION_LOCK_SCREEN", 28),
        ActionSpec("screenshot", "截图", "GLOBAL_ACTION_TAKE_SCREENSHOT", 30),
        ActionSpec("toast", "气泡提示", "Toast（S0 调试）", 26),
    )

    fun byId(id: String): ActionSpec? = specs.firstOrNull { it.id == id }
}

class SemanticActions(private val service: AccessibilityService) {

    fun perform(id: String, reason: String = ""): ActionResult = when (id) {
        "copy" -> nodeAction(AccessibilityNodeInfo.ACTION_COPY, "ACTION_COPY")
        "paste" -> nodeAction(AccessibilityNodeInfo.ACTION_PASTE, "ACTION_PASTE")
        "cut" -> nodeAction(AccessibilityNodeInfo.ACTION_CUT, "ACTION_CUT")
        "selectAll" -> selectAll()
        "setText" -> setText()
        "scrollBackward" -> nodeAction(AccessibilityNodeInfo.ACTION_SCROLL_BACKWARD, "ACTION_SCROLL_BACKWARD")
        "scrollForward" -> nodeAction(AccessibilityNodeInfo.ACTION_SCROLL_FORWARD, "ACTION_SCROLL_FORWARD")
        "imeEnter" -> if (Build.VERSION.SDK_INT >= 30) {
            nodeAction(AccessibilityNodeInfo.AccessibilityAction.ACTION_IME_ENTER.id, "ACTION_IME_ENTER")
        } else {
            ActionResult(false, "API 30 以下没有 ACTION_IME_ENTER")
        }

        "recents" -> global(AccessibilityService.GLOBAL_ACTION_RECENTS, "GLOBAL_ACTION_RECENTS")
        "home" -> global(AccessibilityService.GLOBAL_ACTION_HOME, "GLOBAL_ACTION_HOME")
        "back" -> global(AccessibilityService.GLOBAL_ACTION_BACK, "GLOBAL_ACTION_BACK")
        "notifications" -> global(AccessibilityService.GLOBAL_ACTION_NOTIFICATIONS, "GLOBAL_ACTION_NOTIFICATIONS")
        "quickSettings" -> global(AccessibilityService.GLOBAL_ACTION_QUICK_SETTINGS, "GLOBAL_ACTION_QUICK_SETTINGS")
        "lockScreen" -> if (Build.VERSION.SDK_INT >= 28) {
            global(AccessibilityService.GLOBAL_ACTION_LOCK_SCREEN, "GLOBAL_ACTION_LOCK_SCREEN")
        } else {
            ActionResult(false, "API 28 以下没有 GLOBAL_ACTION_LOCK_SCREEN")
        }

        "screenshot" -> if (Build.VERSION.SDK_INT >= 30) {
            global(AccessibilityService.GLOBAL_ACTION_TAKE_SCREENSHOT, "GLOBAL_ACTION_TAKE_SCREENSHOT")
        } else {
            ActionResult(false, "API 30 以下没有 GLOBAL_ACTION_TAKE_SCREENSHOT")
        }

        "toast" -> {
            Toast.makeText(service, "Kada S0 · ${reason.ifBlank { "气泡" }}", Toast.LENGTH_SHORT).show()
            ActionResult(true, "已弹气泡")
        }

        else -> ActionResult(false, "未知动作 $id")
    }

    private fun global(action: Int, label: String): ActionResult {
        val ok = service.performGlobalAction(action)
        return if (ok) ActionResult(true, "$label 已发出")
        else ActionResult(false, "$label 被系统拒绝（ROM 裁剪 / 当前不允许）")
    }

    private fun nodeAction(action: Int, label: String, args: Bundle? = null): ActionResult {
        val focus = focusedNode() ?: return ActionResult(false, noFocusDetail())

        // 动作有时挂在内层 EditText、有时挂在外层容器：沿父链试到第 8 层。
        var node: AccessibilityNodeInfo? = focus
        var depth = 0
        while (node != null && depth < 8) {
            if (node.performAction(action, args)) {
                return ActionResult(true, "$label 成功（${node.className ?: "?"} / 深度 $depth）")
            }
            node = node.parent
            depth++
        }
        return ActionResult(false, "$label 不被聚焦节点及其父级接受")
    }

    /**
     * 找真正的输入焦点。
     *
     * 顺序有讲究（都踩过坑）：
     * 1. `AccessibilityService.findFocus(FOCUS_INPUT)` 是专门为此提供的接口，最准；
     * 2. 退回来按 `windows` 逐个找——软键盘弹出时 `rootInActiveWindow` 常常是输入法窗口
     *    （里面没有输入焦点），只问它永远得到 null。先看本应用的窗口，免得取到后面
     *    别的 app 留下的焦点节点；
     * 3. 最后才问 `rootInActiveWindow`。
     */
    private fun focusedNode(): AccessibilityNodeInfo? {
        service.findFocus(AccessibilityNodeInfo.FOCUS_INPUT)?.let { return it }

        val appWindows = service.windows.filter { it.type == AccessibilityWindowInfo.TYPE_APPLICATION }
        val ordered = appWindows.sortedByDescending { it.root?.packageName?.toString() == service.packageName }
        for (window in ordered) {
            val root = window.root ?: continue
            root.findFocus(AccessibilityNodeInfo.FOCUS_INPUT)?.let { return it }
        }

        val root = service.rootInActiveWindow ?: return null
        return root.findFocus(AccessibilityNodeInfo.FOCUS_INPUT)
            ?: root.findFocus(AccessibilityNodeInfo.FOCUS_ACCESSIBILITY)
    }

    private fun noFocusDetail(): String {
        val names = service.windows.joinToString(",") { window ->
            val pkg = window.root?.packageName?.toString() ?: "?"
            "$pkg(type=${window.type},focused=${window.isFocused})"
        }
        return "没有聚焦节点（先点进一个输入框 / 列表）；窗口=$names"
    }

    private fun selectAll(): ActionResult {
        val focus = focusedNode() ?: return ActionResult(false, noFocusDetail())
        val length = focus.text?.length ?: 0
        if (length <= 0) return ActionResult(false, "聚焦节点没有文本")

        val args = Bundle().apply {
            putInt(AccessibilityNodeInfo.ACTION_ARGUMENT_SELECTION_START_INT, 0)
            putInt(AccessibilityNodeInfo.ACTION_ARGUMENT_SELECTION_END_INT, length)
        }
        var node: AccessibilityNodeInfo? = focus
        var depth = 0
        while (node != null && depth < 8) {
            if (node.performAction(AccessibilityNodeInfo.ACTION_SET_SELECTION, args)) {
                return ActionResult(true, "已选中 0..$length（${node.className ?: "?"} / 深度 $depth）")
            }
            node = node.parent
            depth++
        }
        return ActionResult(false, "ACTION_SET_SELECTION 不被聚焦节点及其父级接受")
    }

    /**
     * 探针用：把聚焦节点的真实属性与**已登记的动作列表**打出来。
     *
     * 复制 / 剪切 / 粘贴在 `performAction` 里返回 false 时，光看返回值分不清是
     * 「节点不支持」还是「参数不对」，所以直接把 `actionList` 摊开——ROM 裁剪与否一目了然。
     */
    fun describeFocus(): String {
        val focus = focusedNode() ?: return "没有聚焦节点：${noFocusDetail()}"

        val head = buildString {
            append("pkg=${focus.packageName}")
            append(" class=${focus.className}")
            append(" editable=${focus.isEditable} enabled=${focus.isEnabled}")
            append(" focusable=${focus.isFocusable} focused=${focus.isFocused}")
            append(" textLen=${focus.text?.length ?: -1}")
            append(" sel=${focus.textSelectionStart}..${focus.textSelectionEnd}")
        }

        val windows = service.windows.joinToString(",") { window ->
            "${window.root?.packageName ?: "?"}(type=${window.type},focused=${window.isFocused})"
        }

        val chain = buildList {
            var node: AccessibilityNodeInfo? = focus
            var depth = 0
            while (node != null && depth < 4) {
                val ids = node.actionList.joinToString(",") { it.id.toString() }
                add("#$depth ${node.className}[$ids]")
                node = node.parent
                depth++
            }
        }.joinToString(" <- ")

        return "$head | windows=$windows | $chain"
    }

    private fun setText(): ActionResult {
        val args = Bundle().apply {
            putCharSequence(AccessibilityNodeInfo.ACTION_ARGUMENT_SET_TEXT_CHARSEQUENCE, "Kada S0 写入测试")
        }
        val result = nodeAction(AccessibilityNodeInfo.ACTION_SET_TEXT, "ACTION_SET_TEXT", args)
        return if (result.ok) result else ActionResult(false, result.detail + "；SET_TEXT 只在可编辑节点上生效")
    }
}
