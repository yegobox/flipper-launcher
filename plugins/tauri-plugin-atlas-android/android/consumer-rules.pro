# Commands are found by reflection, the worker/receiver by the manifest and
# AtlasNative's native methods by the Rust JNI exports: keep them all.
-keep class com.yegobox.atlas.** { *; }
