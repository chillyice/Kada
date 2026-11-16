package com.kada.spike

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.util.Log
import java.io.File

/**
 * 仅 S0 探针用的控制入口，方便 adb / 自动化脚本改实验开关：
 *
 * ```
 * adb shell am broadcast -a com.kada.spike.CONTROL \
 *   -n com.kada.spike/.ControlReceiver --es op swallow --ez on true
 * adb shell am broadcast -a com.kada.spike.CONTROL \
 *   -n com.kada.spike/.ControlReceiver --es op export
 * ```
 *
 * `exported=true` 是为了让 adb 发得进来，但配了 `android.permission.DUMP`：
 * 只有 shell / system 持有该权限，普通应用广播不进来（docs/安卓版规划.md §8）。
 * 注意：adb 若被换成普通应用身份（非 shell uid）同样会被拦，这是预期行为。
 */
class ControlReceiver : BroadcastReceiver() {

    override fun onReceive(context: Context, intent: Intent) {
        SpikeState.init(context)
        val op = intent.getStringExtra("op") ?: return
        when (op) {
            "swallow" -> setState(op, bool(intent)) { SpikeState.swallowMaster = it }
            "demoRules" -> setState(op, bool(intent)) { SpikeState.demoRules = it }
            "swallowA" -> setState(op, bool(intent)) { SpikeState.swallowA = it }
            "swallowVolume" -> setState(op, bool(intent)) { SpikeState.swallowVolume = it }
            "volumeChord" -> setState(op, bool(intent)) { SpikeState.volumeChord = it }
            "chordSwallowAll" -> setState(op, bool(intent)) { SpikeState.chordSwallowAll = it }
            "tapHoldMs" -> {
                val ms = intent.getIntExtra("ms", SpikeState.tapHoldMs)
                SpikeState.tapHoldMs = ms
                Log.i(SpikeState.TAG, "ctrl op=tapHoldMs ms=${SpikeState.tapHoldMs}")
            }

            "import" -> {
                val name = intent.getStringExtra("file") ?: "incoming.json"
                val merge = intent.getBooleanExtra("merge", true)
                val dir = context.getExternalFilesDir(null) ?: context.filesDir
                val text = runCatching { File(dir, name).readText() }.getOrNull()
                if (text == null) {
                    Log.i(SpikeState.TAG, "ctrl op=import missing=$name")
                } else {
                    val report = RustBridge.importFrom(text, merge)
                    Log.i(
                        SpikeState.TAG,
                        "ctrl op=import file=$name merge=$merge ok=${report?.optBoolean("ok", false)} " +
                            "error=${report?.optString("error")} counts=${report?.optJSONObject("counts")}",
                    )
                }
            }

            "exportConfig" -> {
                val text = RustBridge.exportText()
                val dir = context.getExternalFilesDir(null) ?: context.filesDir
                val file = File(dir, "exported-config.json")
                runCatching { file.writeText(text) }
                Log.i(SpikeState.TAG, "ctrl op=exportConfig path=${file.absolutePath} len=${text.length}")
            }

            "focusField" -> {
                val activity = MainActivity.instance
                if (activity == null) {
                    Log.i(SpikeState.TAG, "ctrl op=focusField activity-not-open")
                } else {
                    activity.runOnUiThread { activity.focusFieldForTest() }
                    Log.i(SpikeState.TAG, "ctrl op=focusField ok")
                }
            }

            "useRust" -> setState(op, bool(intent)) { SpikeState.useRustEngine = it }

            "resetConfig" -> {
                val report = RustBridge.resetToSample(context)
                Log.i(
                    SpikeState.TAG,
                    "ctrl op=resetConfig ok=${report.isNotEmpty()} " +
                        "counts=${RustBridge.json()?.optJSONObject("counts")}",
                )
            }

            "rust" -> {
                val report = RustBridge.reload()
                Log.i(SpikeState.TAG, "ctrl op=rust loaded=${RustBridge.available} report=${report.take(600)}")
            }

            // ---- S1-d 编辑器：与界面同一份 ShortcutStore，adb 侧只代替点按 ----
            "editorAdd" -> {
                RustBridge.init(context)
                val result = ShortcutStore.save(
                    index = null,
                    name = intent.getStringExtra("name") ?: "",
                    trigger = intent.getStringExtra("trigger") ?: "",
                    actionType = intent.getStringExtra("action") ?: "open_url",
                    actionValue = intent.getStringExtra("value") ?: "",
                    enabled = intent.getBooleanExtra("enabled", true),
                )
                Log.i(SpikeState.TAG, "ctrl op=editorAdd ${ShortcutStore.lastResult} error=${result.error}")
            }

            "editorDelete" -> {
                RustBridge.init(context)
                val result = ShortcutStore.delete(intent.getIntExtra("index", -1))
                Log.i(SpikeState.TAG, "ctrl op=editorDelete ${ShortcutStore.lastResult} error=${result.error}")
            }

            "editorCheck" -> {
                RustBridge.init(context)
                val check = ShortcutStore.check(intent.getStringExtra("trigger") ?: "", -1)
                Log.i(
                    SpikeState.TAG,
                    "ctrl op=editorCheck ok=${check.ok} canonical=${check.canonical} kind=${check.kind} " +
                        "normalized=${check.normalized} duplicates=${check.duplicates} error=${check.error}",
                )
            }

            "editorList" -> {
                RustBridge.init(context)
                val list = ShortcutStore.entries()
                Log.i(SpikeState.TAG, "ctrl op=editorList n=${list.size}")
                for (e in list) {
                    Log.i(
                        SpikeState.TAG,
                        "  #${e.index} ${e.name} [${e.triggers.joinToString("|")}] ${e.actionType} " +
                            "enabled=${e.enabled} ${e.actionSummary}",
                    )
                }
            }

            "editorOpen" -> {
                val activity = MainActivity.instance
                if (activity == null) {
                    Log.i(SpikeState.TAG, "ctrl op=editorOpen activity-not-open")
                } else {
                    activity.runOnUiThread { activity.openEditorForTest() }
                    Log.i(SpikeState.TAG, "ctrl op=editorOpen ok")
                }
            }

            // ---- S1-f 自更新：检查 / 下载 / 安装（adb 只代替点按，网络与安装走同一条路） ----
            "checkUpdate" -> {
                val url = intent.getStringExtra("url") ?: Update.MANIFEST_URL
                Thread {
                    Update.check(context, url)
                    Log.i(SpikeState.TAG, "ctrl op=checkUpdate phase=${Update.phase} ${Update.lastResult}")
                }.start()
            }

            "downloadUpdate" -> {
                val info = Update.lastCheck
                if (info == null) {
                    Log.i(SpikeState.TAG, "ctrl op=downloadUpdate no-manifest（先 checkUpdate）")
                } else {
                    Thread {
                        Update.download(context, info)
                        Log.i(SpikeState.TAG, "ctrl op=downloadUpdate phase=${Update.phase} ${Update.lastResult}")
                    }.start()
                }
            }

            "installUpdate" -> {
                val result = Update.install(context)
                Log.i(
                    SpikeState.TAG,
                    "ctrl op=installUpdate phase=${Update.phase} ${Update.lastResult} " +
                        "error=${result.exceptionOrNull()?.message}",
                )
            }

            "decide" -> {
                val key = intent.getStringExtra("key") ?: "F8"
                val down = intent.getBooleanExtra("down", true)
                val repeat = intent.getBooleanExtra("repeat", false)
                val mods = intent.getStringExtra("mods") ?: ""
                if (intent.getBooleanExtra("reset", false)) KeyFilterService.instance?.resetForTest()
                val d = KeyFilterService.instance?.decideForTest(key, down, repeat, mods)
                Log.i(
                    SpikeState.TAG,
                    "ctrl op=decide key=$key down=$down repeat=$repeat mods=$mods " +
                        "-> swallow=${d?.swallow} fired=${d?.fired?.joinToString(",") { it.trigger }} " +
                        "note=${d?.note}",
                )
            }

            "startService" -> {
                context.startForegroundService(Intent(context, HeartbeatService::class.java))
                Log.i(SpikeState.TAG, "ctrl op=startService")
            }

            "stopService" -> {
                context.stopService(Intent(context, HeartbeatService::class.java))
                Log.i(SpikeState.TAG, "ctrl op=stopService")
            }

            "fire" -> {
                val actionId = intent.getStringExtra("action") ?: "toast"
                KeyFilterService.instance?.dispatchForTest(actionId)
                    ?: Log.i(SpikeState.TAG, "ctrl op=fire action=$actionId a11y-not-connected")
                Log.i(SpikeState.TAG, "ctrl op=fire action=$actionId")
            }

            "export" -> {
                val file = SpikeReport.write(context)
                Log.i(SpikeState.TAG, "ctrl op=export path=${file.absolutePath}")
            }

            "snapshot" -> {
                SpikeState.logSnapshot()
                Log.i(SpikeState.TAG, "ctrl op=snapshot")
            }

            "focusInfo" -> {
                KeyFilterService.instance?.describeFocusForTest()
                    ?: Log.i(SpikeState.TAG, "ctrl op=focusInfo a11y-not-connected")
            }

            "clear" -> {
                SpikeState.clear()
                Log.i(SpikeState.TAG, "ctrl op=clear")
            }

            else -> Log.i(SpikeState.TAG, "ctrl op=unknown op=$op")
        }
    }

    private fun bool(intent: Intent): Boolean = intent.getBooleanExtra("on", true)

    private fun setState(op: String, value: Boolean, apply: (Boolean) -> Unit) {
        apply(value)
        Log.i(SpikeState.TAG, "ctrl op=$op on=$value")
    }
}
