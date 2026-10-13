# JNA reaches the generated bindings' structures and callbacks by reflection.
-keep class com.sun.jna.** { *; }
-keep class * extends com.sun.jna.** { *; }
-keep class cc.blit.koan.ffi.** { *; }
-dontwarn java.awt.**
