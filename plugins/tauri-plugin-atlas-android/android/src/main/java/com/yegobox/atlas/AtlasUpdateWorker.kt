package com.yegobox.atlas

import android.content.Context
import androidx.work.Constraints
import androidx.work.ExistingPeriodicWorkPolicy
import androidx.work.NetworkType
import androidx.work.PeriodicWorkRequestBuilder
import androidx.work.WorkManager
import androidx.work.Worker
import androidx.work.WorkerParameters
import org.json.JSONObject
import java.io.File
import java.util.concurrent.TimeUnit

/** Keeps installed apps current even when nobody opens Atlas. */
class AtlasUpdateWorker(ctx: Context, params: WorkerParameters) : Worker(ctx, params) {
    override fun doWork(): Result {
        val ctx = applicationContext
        val out = try {
            JSONObject(
                AtlasNative.backgroundCheck(ctx.filesDir.absolutePath, ctx.cacheDir.absolutePath, Packages.installedJson(ctx))
            )
        } catch (e: Throwable) {
            return Result.retry()
        }
        val apks = out.optJSONArray("apks") ?: return Result.success()
        // Atlas's own APK is listed last by the Rust side: installing it ends this process.
        for (i in 0 until apks.length()) {
            val a = apks.getJSONObject(i)
            try {
                Installer.install(ctx, File(a.getString("path")), a.optString("label", "An app"), null)
            } catch (e: Exception) {
                // Try the rest; the next run retries this one.
            }
        }
        return if (out.has("error")) Result.retry() else Result.success()
    }

    companion object {
        private const val NAME = "atlas-updates"

        fun schedule(ctx: Context, hours: Long) {
            val wm = WorkManager.getInstance(ctx)
            if (hours <= 0) {
                wm.cancelUniqueWork(NAME)
                return
            }
            val req = PeriodicWorkRequestBuilder<AtlasUpdateWorker>(hours, TimeUnit.HOURS)
                .setConstraints(Constraints.Builder().setRequiredNetworkType(NetworkType.CONNECTED).build())
                .build()
            wm.enqueueUniquePeriodicWork(NAME, ExistingPeriodicWorkPolicy.UPDATE, req)
        }
    }
}
