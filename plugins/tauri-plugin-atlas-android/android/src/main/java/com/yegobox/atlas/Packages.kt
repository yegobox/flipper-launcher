package com.yegobox.atlas

import android.content.Context
import android.content.pm.PackageInfo
import android.content.pm.PackageManager
import android.os.Build
import org.json.JSONObject

/** Installed-package lookups (Atlas holds QUERY_ALL_PACKAGES). */
object Packages {
    fun info(ctx: Context, pkg: String): PackageInfo? = try {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            ctx.packageManager.getPackageInfo(pkg, PackageManager.PackageInfoFlags.of(0))
        } else {
            @Suppress("DEPRECATION")
            ctx.packageManager.getPackageInfo(pkg, 0)
        }
    } catch (e: PackageManager.NameNotFoundException) {
        null
    }

    fun code(info: PackageInfo): Long =
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.P) info.longVersionCode
        else @Suppress("DEPRECATION") info.versionCode.toLong()

    /** {"package": versionCode, ...} for every installed package; the Rust check picks what it needs. */
    fun installedJson(ctx: Context): String {
        val pm = ctx.packageManager
        val all = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            pm.getInstalledPackages(PackageManager.PackageInfoFlags.of(0))
        } else {
            @Suppress("DEPRECATION")
            pm.getInstalledPackages(0)
        }
        val out = JSONObject()
        for (p in all) out.put(p.packageName, code(p))
        return out.toString()
    }
}
