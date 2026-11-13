package com.kada.spike

import android.accessibilityservice.AccessibilityService
import android.content.ClipData
import android.content.ClipboardManager
import android.content.Intent
import android.net.Uri
import android.os.Build
import android.util.Log
import org.json.JSONArray
import org.json.JSONObject
import java.util.concurrent.Executors

/**
 * 动作执行器（S1-a，无 root 档落地集）。
 *
 * **支持哪些类型由 Rust 判定**（`crates/kada-android/src/action_support.rs`），这里只认
 * 那份清单里的类型；清单外的动作一律跳过并留痕（不占执行记录，同桌面 `Action::Disabled`
 * 的「跳过」口径）。新增支持类型时两处同步改，Rust 是唯一事实来源。
 *
 * `semantic`（语义动作）的子操作名也以 Rust（`kada_core::SemanticOperation`）为准，
 * 与探针 `ActionCatalog` 的 id 逐字相同，**不在这边做第二张映射表**。
 *
 * 线程：整条链跑在单线程执行器上（`pause_ms` 要真的等）；无障碍节点动作是 binder 调用，
 * 不必回主线程。**钩子回调里绝不执行动作**——`onKeyEvent` 只做判定并 post 出去。
 */
class ActionRunner(private val service: AccessibilityService) {

    private val semantic = SemanticActions(service)
    private val executor = Executors.newSingleThreadExecutor { r ->
        Thread(r, "kada-actions").apply { isDaemon = true }
    }

    fun run(fired: RustBridge.Fired, note: String) {
        executor.execute {
            val report = Outcome()
            runChain(fired.actions, 0, report)
            val ok = report.failed == 0
            val detail = buildString {
                append("${fired.trigger} → ${fired.name.ifBlank { "(未命名)" }}")
                append("：执行 ${report.executed} 步")
                if (report.skipped > 0) append("，跳过 ${report.skipped} 步（本机不支持）")
                if (report.failed > 0) append("，失败 ${report.failed} 步：${report.firstError}")
                if (fired.unsupported.isNotEmpty()) {
                    append(" · 不支持类型=${fired.unsupported.joinToString(",")}")
                }
            }
            SpikeState.recordAction("rust:${fired.trigger}", ok, detail, note)
            SpikeState.noteRustFire(fired.trigger, fired.name, ok, detail)
            Log.i(SpikeState.TAG, "rust fire $detail")
        }
    }

    private fun runChain(actions: JSONArray, depth: Int, report: Outcome) {
        if (depth > 8) {
            report.fail("动作嵌套超过 8 层")
            return
        }
        for (i in 0 until actions.length()) {
            val action = actions.optJSONObject(i) ?: continue
            step(action, depth, report)
        }
    }

    private fun step(action: JSONObject, depth: Int, report: Outcome) {
        when (val type = action.optString("type")) {
            "open_url" -> {
                val url = action.optString("url")
                try {
                    val intent = Intent(Intent.ACTION_VIEW, Uri.parse(url)).apply {
                        addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
                    }
                    service.startActivity(intent)
                    report.ok()
                } catch (t: Throwable) {
                    report.fail("打开网址失败：$url（${t.message}）")
                }
            }

            "text" -> {
                // 文本落地：写剪贴板 + 粘贴到当前输入框（规划 §4 方案 ③ 的底座）。
                val text = action.optString("text")
                if (text.isEmpty()) {
                    report.fail("文本动作为空")
                    return
                }
                if (!setClipboard(text)) {
                    report.fail("写剪贴板失败")
                    return
                }
                val pasted = semantic.perform("paste")
                if (pasted.ok) report.ok() else report.fail("粘贴失败：${pasted.detail}")
            }

            "clipboard" -> {
                val op = action.optJSONObject("operation") ?: JSONObject()
                when (op.optString("op")) {
                    "set_clipboard" -> {
                        val text = op.optString("text")
                        if (setClipboard(text)) report.ok() else report.fail("写剪贴板失败")
                    }

                    "tempstr" -> {
                        val text = op.optString("text")
                        if (!setClipboard(text)) {
                            report.fail("写剪贴板失败")
                            return
                        }
                        val pasted = semantic.perform("paste")
                        if (pasted.ok) report.ok() else report.fail("粘贴失败：${pasted.detail}")
                    }

                    else -> report.skip("clipboard/${op.optString("op")}")
                }
            }

            "semantic" -> {
                // 语义动作：配置里 `operation` 是短名（`kada_core::SemanticOperation`），
                // 与 `ActionCatalog` 的 id 逐字相同，直接查表；查不到 = 名字分叉，必须报错别猜。
                val op = action.optString("operation")
                val spec = ActionCatalog.byId(op)
                if (spec == null) {
                    report.fail("未知语义动作「${op.ifEmpty { "?" }}」（core 与探针的动作名不一致）")
                    return
                }
                if (Build.VERSION.SDK_INT < spec.minSdk) {
                    report.fail("${spec.label}需要 API ${spec.minSdk}（本机 ${Build.VERSION.SDK_INT}）")
                    return
                }
                val result = semantic.perform(spec.id)
                if (result.ok) report.ok() else report.fail("${spec.label}失败：${result.detail}")
            }

            "pause_ms" -> {
                val ms = action.optLong("ms").coerceIn(0, 60_000)
                try {
                    Thread.sleep(ms)
                    report.ok()
                } catch (t: InterruptedException) {
                    Thread.currentThread().interrupt()
                    report.fail("等待被中断")
                }
            }

            "group" -> {
                // 分组是纯组织结构（桌面同款语义）：成员原地顺序执行。
                runChain(action.optJSONArray("actions") ?: JSONArray(), depth + 1, report)
            }

            "disabled" -> {
                // 禁用：整条跳过，不执行、不占记录（桌面同款）。
                report.skip("disabled")
            }

            else -> report.skip(type.ifEmpty { "unknown" })
        }
    }

    private fun setClipboard(text: String): Boolean = runCatching {
        val manager = service.getSystemService(ClipboardManager::class.java)
        manager.setPrimaryClip(ClipData.newPlainText("Kada", text))
        true
    }.getOrDefault(false)

    private class Outcome {
        var executed = 0
        var skipped = 0
        var failed = 0
        var firstError = ""

        fun ok() {
            executed++
        }

        fun skip(kind: String) {
            skipped++
            Log.i(SpikeState.TAG, "rust action skipped: $kind")
        }

        fun fail(message: String) {
            failed++
            if (firstError.isEmpty()) firstError = message
        }
    }
}
