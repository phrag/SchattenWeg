# JNA + UniFFI bindings are reached reflectively; R8 must not strip or
# rename them.
-keep class com.sun.jna.** { *; }
-keep class * implements com.sun.jna.** { *; }
-keep class uniffi.** { *; }
-dontwarn java.awt.**

# Release builds drop debug/verbose logging entirely, so a stray Log.d can
# never put a tapped point or a position into logcat on a user's phone.
-assumenosideeffects class android.util.Log {
    public static int d(...);
    public static int v(...);
}
