package com.yegobox.atlas

/** Atlas's Rust library: the same catalog, download and signature code the window uses. */
object AtlasNative {
    init {
        System.loadLibrary("yegobox_launcher_lib")
    }

    /**
     * Fetches the catalog, downloads every app (and Atlas itself) that is newer
     * than `installedJson` ({"package": versionCode}), and verifies each file.
     * Returns {"apks":[{"path","pkg","label"}], "error":?}.
     */
    @JvmStatic
    external fun backgroundCheck(filesDir: String, cacheDir: String, installedJson: String): String
}
