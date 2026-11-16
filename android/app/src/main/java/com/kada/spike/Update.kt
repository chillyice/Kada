package com.kada.spike

import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.content.pm.PackageInstaller
import android.net.Uri
import android.os.Build
import android.provider.Settings
import android.util.Log
import org.json.JSONObject
import java.io.File
import java.io.InputStream
import java.net.HttpURLConnection
import java.net.URL
import java.security.MessageDigest

/**
 * 侧载自更新（S1-f）：清单检查 → 下载 → 交给系统安装器。
 *
 * 为什么不复用桌面的 `tauri-plugin-updater`：它不支持 Android。Play 渠道走商店，
 * 侧载只有一条正路——`PackageInstaller` 会话 + 用户确认（规划 §8）。
 *
 * 三条边界：
 * 1. **清单是唯一入口**：`https` 上的一个 JSON（`versionName` / `versionCode` / `url` / `sha256` / `notes`）。
 *    版本比较用 `versionCode`（整数，唯一可靠），`versionName` 只用来显示。
 * 2. **下载必须校验**：清单给了 `sha256` 就逐字节算一遍再比；不给就在结果里明说「未校验」，
 *    绝不假装验过。
 * 3. **安装必须用户点头**：先看 `canRequestPackageInstalls()`；没授权就回报 `need_permission`，
 *    由界面引导去系统设置——**不许静默失败**，也不去猜系统弹窗为什么没出来。
 *
 * 状态都放在这个 object 里（`phase` / `lastResult` / `lastCheck`），报告导出与 adb 断言读同一份。
 */
object Update {

    /** 侧载清单地址（真实发布用；测试可用 `file://` 覆盖）。 */
    const val MANIFEST_URL = "https://ihomy.top/files/kada/android/latest.json"

    /** 系统安装器回执的广播 action（`UpdateReceiver` 收）。 */
    const val ACTION_STATUS = "com.kada.spike.INSTALL_STATUS"

    private const val DIR = "update"
    private const val APK = "kada-update.apk"
    private const val TAG = SpikeState.TAG

    /** 清单内容（字段与 `latest.json` 一一对应）。 */
    data class Info(
        val versionName: String,
        val versionCode: Int,
        val url: String,
        val sha256: String,
        val notes: String,
    )

    /**
     * 当前阶段：`idle` / `checking` / `latest` / `available` / `downloading` / `ready` /
     * `installing` / `need_permission` / `failed`。
     */
    @Volatile
    var phase: String = "idle"
        private set

    @Volatile
    var lastResult: String = ""
        private set

    @Volatile
    var lastCheck: Info? = null
        private set

    /** 清单里的版本比本机新（`versionCode` 比较）。 */
    @Volatile
    var newer: Boolean = false
        private set

    /** 下载进度（KB；`file://` 也走同一条路）。 */
    @Volatile
    var progressKb: Int = 0
        private set

    fun currentName(context: Context): String = runCatching {
        context.packageManager.getPackageInfo(context.packageName, 0).versionName
    }.getOrNull() ?: "?"

    @Suppress("DEPRECATION")
    fun currentCode(context: Context): Int = runCatching {
        val info = context.packageManager.getPackageInfo(context.packageName, 0)
        if (Build.VERSION.SDK_INT >= 28) info.longVersionCode.toInt() else info.versionCode
    }.getOrDefault(0)

    /** 下载好的安装包（没有就是 null）。 */
    fun localApk(context: Context): File? = File(File(context.cacheDir, DIR), APK)
        .takeIf { it.isFile && it.length() > 0 }

    fun canInstall(context: Context): Boolean = context.packageManager.canRequestPackageInstalls()

    /**
     * 纯函数：解析清单。缺字段就抛——**不猜默认值**（缺 `versionCode` 的清单会永远「没有新版本」，
     * 这种静默失败最坑）。
     */
    fun parse(json: String): Info {
        val obj = JSONObject(json)
        val versionName = obj.optString("versionName").ifEmpty { obj.optString("version") }
        val versionCode = obj.optInt("versionCode", 0)
        if (versionName.isEmpty()) throw IllegalArgumentException("清单缺 versionName")
        if (versionCode <= 0) throw IllegalArgumentException("清单缺 versionCode（正整数）")
        val url = obj.optString("url")
        if (url.isEmpty()) throw IllegalArgumentException("清单缺 url")
        return Info(
            versionName = versionName,
            versionCode = versionCode,
            url = url,
            sha256 = obj.optString("sha256").lowercase(),
            notes = obj.optString("notes"),
        )
    }

    /** 纯函数：清单版本是否比本机新。 */
    fun isNewer(info: Info, currentCode: Int): Boolean = info.versionCode > currentCode

    /** 拉清单 → 解析 → 比较。`url` 可覆盖（adb / 测试用本地清单）。 */
    fun check(context: Context, url: String = MANIFEST_URL): Result<Info> {
        phase = "checking"
        progressKb = 0
        lastResult = "检查中：$url"
        val result = runCatching {
            val info = parse(fetchText(url))
            lastCheck = info
            newer = isNewer(info, currentCode(context))
            phase = if (newer) "available" else "latest"
            lastResult = if (newer) {
                "发现新版本 ${info.versionName}（当前 ${currentName(context)}）"
            } else {
                "已是最新（${currentName(context)}）"
            }
            info
        }
        result.exceptionOrNull()?.let {
            lastCheck = null
            newer = false
            phase = "failed"
            lastResult = "检查更新失败：${it.message ?: it::class.java.simpleName}"
        }
        Log.i(TAG, "update check ${lastResult}")
        return result
    }

    /** 下载并校验 sha256，落到 `cacheDir/update/kada-update.apk`。 */
    fun download(context: Context, info: Info): Result<File> {
        phase = "downloading"
        progressKb = 0
        lastResult = "下载中：${info.url}"
        val result = runCatching {
            val dir = File(context.cacheDir, DIR).apply { mkdirs() }
            val dest = File(dir, APK)
            if (dest.exists()) dest.delete()

            val digest = MessageDigest.getInstance("SHA-256")
            var total = 0L
            openStream(info.url).use { input ->
                dest.outputStream().use { output ->
                    val buf = ByteArray(64 * 1024)
                    while (true) {
                        val n = input.read(buf)
                        if (n < 0) break
                        output.write(buf, 0, n)
                        digest.update(buf, 0, n)
                        total += n
                        progressKb = (total / 1024).toInt()
                    }
                }
            }
            if (total <= 0) throw IllegalStateException("下载内容为空（0 字节）")

            val sha = digest.digest().joinToString("") { "%02x".format(it.toInt() and 0xFF) }
            if (info.sha256.isNotEmpty() && !info.sha256.equals(sha, ignoreCase = true)) {
                dest.delete()
                throw IllegalStateException(
                    "sha256 不匹配（清单 ${info.sha256.take(12)}… / 实际 ${sha.take(12)}…），已删掉下载的文件",
                )
            }
            phase = "ready"
            lastResult = buildString {
                append("已下载 ${total / 1024} KB")
                append(if (info.sha256.isEmpty()) "（清单没给 sha256，未校验）" else "（sha256 已校验）")
            }
            dest
        }
        result.exceptionOrNull()?.let {
            phase = "failed"
            lastResult = "下载失败：${it.message ?: it::class.java.simpleName}"
        }
        Log.i(TAG, "update download ${lastResult}")
        return result
    }

    /**
     * 交给系统安装器（`PackageInstaller` 会话，不自己造 `file://` Intent——
     * Android 7+ 的 `FileUriExposedException` 就是那条路的坑）。
     *
     * 提交后系统会弹自己的确认框；用户点头才真的装。没授权「安装未知应用」时
     * 只回报 `need_permission`，由界面去引导。
     *
     * **写会话在后台线程做**：30 MB 的 APK 拷进会话要好几秒，调用方多半在主线程
     * （按钮点击 / 广播回调），在这一层拷就是 ANR。返回成功只代表「已开始」，
     * 后续阶段看 [phase] / [lastResult]。
     */
    fun install(context: Context): Result<Unit> {
        val apk = localApk(context)
        if (apk == null) {
            phase = "failed"
            lastResult = "安装失败：还没有下载好的安装包"
            Log.i(TAG, "update install ${lastResult}")
            return Result.failure(IllegalStateException(lastResult))
        }
        if (!canInstall(context)) {
            phase = "need_permission"
            lastResult = "需要先允许「安装未知应用」（系统限制，不能代劳）"
            Log.i(TAG, "update install ${lastResult}")
            return Result.failure(SecurityException(lastResult))
        }

        phase = "installing"
        lastResult = "正在交给系统安装器…"
        Thread {
            val result = runCatching { commitSession(context, apk) }
            result.exceptionOrNull()?.let {
                phase = "failed"
                lastResult = "安装失败：${it.message ?: it::class.java.simpleName}"
            } ?: run {
                lastResult = "已提交安装（系统安装器的确认框里点「安装」才生效）"
            }
            Log.i(TAG, "update install ${lastResult}")
        }.start()
        return Result.success(Unit)
    }

    private fun commitSession(context: Context, apk: java.io.File) {
        val installer = context.packageManager.packageInstaller
        val params = PackageInstaller.SessionParams(PackageInstaller.SessionParams.MODE_FULL_INSTALL).apply {
            setAppPackageName(context.packageName)
            setSize(apk.length())
        }
        val sessionId = installer.createSession(params)
        installer.openSession(sessionId).use { session ->
            apk.inputStream().use { input ->
                session.openWrite("kada", 0, apk.length()).use { output ->
                    input.copyTo(output, 64 * 1024)
                    session.fsync(output)
                }
            }
            session.commit(statusIntent(context).intentSender)
        }
    }

    /** 引导去「允许安装未知应用」；没有这个设置页就退回安全设置。 */
    fun openInstallPermission(context: Context) {
        val direct = Intent(
            Settings.ACTION_MANAGE_UNKNOWN_APP_SOURCES,
            Uri.parse("package:${context.packageName}"),
        ).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
        runCatching { context.startActivity(direct) }.onFailure {
            runCatching {
                context.startActivity(Intent(Settings.ACTION_SECURITY_SETTINGS).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
            }
        }
    }

    /** 系统安装器的回执（`UpdateReceiver` 转进来）。 */
    fun onInstallStatus(context: Context, intent: Intent) {
        val status = intent.getIntExtra(PackageInstaller.EXTRA_STATUS, Int.MIN_VALUE)
        val message = intent.getStringExtra(PackageInstaller.EXTRA_STATUS_MESSAGE).orEmpty()
        when (status) {
            PackageInstaller.STATUS_PENDING_USER_ACTION -> {
                // 少数 ROM 会要求由发起方拉起确认 UI：系统把 Intent 塞在 EXTRA_INTENT 里。
                val confirm = if (Build.VERSION.SDK_INT >= 33) {
                    intent.getParcelableExtra(Intent.EXTRA_INTENT, Intent::class.java)
                } else {
                    @Suppress("DEPRECATION")
                    intent.getParcelableExtra(Intent.EXTRA_INTENT)
                }
                if (confirm != null) {
                    confirm.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
                    runCatching { context.startActivity(confirm) }
                    phase = "installing"
                    lastResult = "等待用户在系统安装器里确认"
                }
            }

            PackageInstaller.STATUS_SUCCESS -> {
                phase = "installed"
                lastResult = "安装成功（新版本下次启动生效）"
            }

            PackageInstaller.STATUS_FAILURE_ABORTED -> {
                phase = "ready"
                lastResult = "安装被取消（安装包还留着，可以再试）"
            }

            else -> {
                phase = "failed"
                lastResult = "安装失败：status=$status $message"
            }
        }
        Log.i(TAG, "update install status=${lastResult}")
    }

    // ------------------------------------------------------------------
    // 网络 / 读取（`file://` 也走这里：测试用本地清单，生产只用 https）
    // ------------------------------------------------------------------

    private fun fetchText(url: String): String =
        openStream(url).use { it.readBytes().decodeToString() }

    private fun openStream(url: String): InputStream {
        val conn = URL(url).openConnection()
        if (conn is HttpURLConnection) {
            conn.connectTimeout = 8_000
            conn.readTimeout = 20_000
            conn.instanceFollowRedirects = true
            conn.setRequestProperty("Cache-Control", "no-cache")
            val code = conn.responseCode
            if (code !in 200..299) throw IllegalStateException("HTTP $code（$url）")
        }
        return conn.getInputStream()
    }

    private fun statusIntent(context: Context): PendingIntent {
        val intent = Intent(context, UpdateReceiver::class.java).setAction(ACTION_STATUS)
        val flags = PendingIntent.FLAG_UPDATE_CURRENT or
            if (Build.VERSION.SDK_INT >= 31) PendingIntent.FLAG_MUTABLE else 0
        return PendingIntent.getBroadcast(context, 4201, intent, flags)
    }
}
