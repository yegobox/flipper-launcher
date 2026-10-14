package com.yegobox.atlas

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent

/** PackageInstaller reports each session's result here. */
class InstallResultReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        Installer.onResult(context, intent)
    }
}
