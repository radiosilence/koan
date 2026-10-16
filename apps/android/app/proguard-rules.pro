# uniffi's generated bindings reach the engine through JNA by reflection, and
# the engine calls back into the rustls-platform-verifier component by name.
-keep class com.sun.jna.** { *; }
-keep class * implements com.sun.jna.** { *; }
-keep class uniffi.** { *; }
-keep class org.rustls.platformverifier.** { *; }
-keep class cc.blit.koan.Native { *; }
-dontwarn java.awt.*
