# JNA + UniFFI bindings are reached reflectively; R8 must not strip or
# rename them.
-keep class com.sun.jna.** { *; }
-keep class * implements com.sun.jna.** { *; }
-keep class uniffi.** { *; }
-dontwarn java.awt.**
