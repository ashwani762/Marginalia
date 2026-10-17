# JNI: the native library looks these up by name.
-keep class app.marginalia.Native { *; }
-keep interface app.marginalia.NativeListener { *; }

-keep interface app.marginalia.LlmGenerator { *; }
-keepclassmembers class * implements app.marginalia.LlmGenerator { *; }
-keep class com.google.ai.edge.litertlm.** { *; }

# Speaker separation: the native library builds these objects and calls the callback by name.
-keep class com.k2fsa.sherpa.onnx.** { *; }
-keepclassmembers class app.marginalia.Diarizer$* { *; }
