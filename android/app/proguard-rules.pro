# Keep the JNI entry points reachable; the whole protocol core is driven from
# Kotlin through com.mykvm.receiver.core.NativeCore.
-keepclasseswithmembernames class com.mykvm.receiver.core.NativeCore {
    native <methods>;
}
-keep class com.mykvm.receiver.core.NativeCore { *; }