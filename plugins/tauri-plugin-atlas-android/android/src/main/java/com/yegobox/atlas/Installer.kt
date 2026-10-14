package com.yegobox.atlas

import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.content.pm.PackageInstaller
import android.os.Build
import java.io.File
import java.util.concurrent.ConcurrentHashMap

/**
 * Installs an APK through a PackageInstaller session. On Android 12+ an update
 * of an app Atlas installed (or of Atlas itself) goes through without a tap;
 * otherwise Android asks: in the foreground the system dialog opens straight
 * away, in the background a notification asks for the tap.
 */
object Installer {
    interface Callback {
        fun done(ok: Boolean, message: String?)
    }

    const val EXTRA_SESSION = "com.yegobox.atlas.SESSION"
    private val callbacks = ConcurrentHashMap<Int, Callback>()
    private val labels = ConcurrentHashMap<Int, String>()

    /** `callback` null = background (worker): a needed confirmation becomes a notification. */
    fun install(ctx: Context, apk: File, label: String, callback: Callback?) {
        val installer = ctx.packageManager.packageInstaller
        val params = PackageInstaller.SessionParams(PackageInstaller.SessionParams.MODE_FULL_INSTALL)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            params.setRequireUserAction(PackageInstaller.SessionParams.USER_ACTION_NOT_REQUIRED)
        }
        val id = installer.createSession(params)
        labels[id] = label
        if (callback != null) callbacks[id] = callback
        try {
            installer.openSession(id).use { session ->
                session.openWrite("base.apk", 0, apk.length()).use { out ->
                    apk.inputStream().use { it.copyTo(out) }
                    session.fsync(out)
                }
                val intent = Intent(ctx, InstallResultReceiver::class.java).putExtra(EXTRA_SESSION, id)
                // The system adds the result extras, so the PendingIntent must be mutable on 12+.
                val flags = PendingIntent.FLAG_UPDATE_CURRENT or
                    (if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) PendingIntent.FLAG_MUTABLE else 0)
                session.commit(PendingIntent.getBroadcast(ctx, id, intent, flags).intentSender)
            }
        } catch (e: Exception) {
            callbacks.remove(id)
            labels.remove(id)
            installer.abandonSession(id)
            throw e
        }
    }

    fun onResult(ctx: Context, intent: Intent) {
        val id = intent.getIntExtra(EXTRA_SESSION, -1)
        when (val status = intent.getIntExtra(PackageInstaller.EXTRA_STATUS, PackageInstaller.STATUS_FAILURE)) {
            PackageInstaller.STATUS_PENDING_USER_ACTION -> {
                val confirm: Intent? = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
                    intent.getParcelableExtra(Intent.EXTRA_INTENT, Intent::class.java)
                } else {
                    @Suppress("DEPRECATION")
                    intent.getParcelableExtra(Intent.EXTRA_INTENT)
                }
                if (confirm == null) return
                if (callbacks.containsKey(id)) {
                    // Started from the Atlas window: show the system dialog now.
                    ctx.startActivity(confirm.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
                } else {
                    Notifications.needsTap(ctx, id, labels[id] ?: "An app", confirm)
                }
            }
            PackageInstaller.STATUS_SUCCESS -> {
                labels.remove(id)
                Notifications.clear(ctx, id)
                callbacks.remove(id)?.done(true, null)
            }
            else -> {
                labels.remove(id)
                val msg = intent.getStringExtra(PackageInstaller.EXTRA_STATUS_MESSAGE) ?: "install failed ($status)"
                callbacks.remove(id)?.done(false, msg)
            }
        }
    }
}
