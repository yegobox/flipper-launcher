package com.yegobox.atlas

import android.Manifest
import android.app.Activity
import android.content.ActivityNotFoundException
import android.content.Intent
import android.net.Uri
import android.os.Build
import android.provider.Settings
import app.tauri.annotation.Command
import app.tauri.annotation.InvokeArg
import app.tauri.annotation.Permission
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin
import java.io.File

@InvokeArg
class PkgArgs {
    lateinit var pkg: String
}

@InvokeArg
class InstallArgs {
    lateinit var path: String
    var label: String = "An app"
}

@InvokeArg
class ScheduleArgs {
    var hours: Long = 6
}

/** What Atlas (Rust) asks Android to do. Registered as "com.yegobox.atlas"/"AtlasPlugin". */
@TauriPlugin(
    permissions = [Permission(strings = [Manifest.permission.POST_NOTIFICATIONS], alias = "notifications")]
)
class AtlasPlugin(private val activity: Activity) : Plugin(activity) {

    @Command
    fun installedVersion(invoke: Invoke) {
        val args = invoke.parseArgs(PkgArgs::class.java)
        val info = Packages.info(activity, args.pkg)
        val out = JSObject()
        out.put("installed", info != null)
        if (info != null) {
            out.put("versionCode", Packages.code(info))
            out.put("versionName", info.versionName)
        }
        invoke.resolve(out)
    }

    @Command
    fun installApk(invoke: Invoke) {
        val args = invoke.parseArgs(InstallArgs::class.java)
        val apk = File(args.path)
        if (!apk.isFile) {
            invoke.reject("APK not found: ${args.path}")
            return
        }
        try {
            Installer.install(activity, apk, args.label, object : Installer.Callback {
                override fun done(ok: Boolean, message: String?) {
                    if (ok) invoke.resolve() else invoke.reject(message ?: "install failed")
                }
            })
        } catch (e: Exception) {
            invoke.reject("could not start the install: ${e.message}", e)
        }
    }

    @Command
    fun launch(invoke: Invoke) {
        val args = invoke.parseArgs(PkgArgs::class.java)
        val intent = activity.packageManager.getLaunchIntentForPackage(args.pkg)
        if (intent == null) {
            invoke.reject("${args.pkg} is not installed")
            return
        }
        activity.startActivity(intent.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
        invoke.resolve()
    }

    @Command
    fun openPlay(invoke: Invoke) {
        val args = invoke.parseArgs(PkgArgs::class.java)
        val market = Intent(Intent.ACTION_VIEW, Uri.parse("market://details?id=${args.pkg}"))
        try {
            activity.startActivity(market.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
        } catch (e: ActivityNotFoundException) {
            // No Play Store on this device: the web listing still works.
            val web = Intent(Intent.ACTION_VIEW, Uri.parse("https://play.google.com/store/apps/details?id=${args.pkg}"))
            activity.startActivity(web.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
        }
        invoke.resolve()
    }

    @Command
    fun canInstall(invoke: Invoke) {
        val allowed = Build.VERSION.SDK_INT < Build.VERSION_CODES.O ||
            activity.packageManager.canRequestPackageInstalls()
        invoke.resolve(JSObject().put("value", allowed))
    }

    @Command
    fun openInstallSettings(invoke: Invoke) {
        val intent = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            Intent(Settings.ACTION_MANAGE_UNKNOWN_APP_SOURCES, Uri.parse("package:${activity.packageName}"))
        } else {
            Intent(Settings.ACTION_SECURITY_SETTINGS)
        }
        activity.startActivity(intent.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
        invoke.resolve()
    }

    @Command
    fun paths(invoke: Invoke) {
        val out = JSObject()
        out.put("filesDir", activity.filesDir.absolutePath)
        out.put("cacheDir", activity.cacheDir.absolutePath)
        out.put("packageName", activity.packageName)
        invoke.resolve(out)
    }

    @Command
    fun scheduleUpdates(invoke: Invoke) {
        val args = invoke.parseArgs(ScheduleArgs::class.java)
        AtlasUpdateWorker.schedule(activity.applicationContext, args.hours)
        invoke.resolve()
    }
}
