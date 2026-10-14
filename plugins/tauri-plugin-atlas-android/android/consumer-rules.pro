# Commands are found by reflection, the worker/receiver by the manifest and
# AtlasNative's native methods by the Rust JNI exports: keep them all.
-keep class com.yegobox.atlas.** { *; }

# WorkManager creates its Room database by class name (WorkDatabase_Impl) when
# the app process starts. Release builds' R8 stripped it, so every release APK
# crashed before Atlas's own code ran ("Failed to create an instance of class
# androidx.work.impl.WorkDatabase"). Debug builds don't shrink, so they worked.
-keep class * extends androidx.room.RoomDatabase { <init>(); }
-keep class androidx.work.impl.WorkDatabase_Impl { *; }
-keepnames class androidx.work.impl.WorkDatabase
-keep class * extends androidx.work.ListenableWorker { <init>(...); }
