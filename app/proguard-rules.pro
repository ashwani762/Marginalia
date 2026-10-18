# JNI: the native library looks these up by name.
-keep class app.marginalia.Native { *; }
-keep interface app.marginalia.NativeListener { *; }

-keep interface app.marginalia.LlmGenerator { *; }
-keepclassmembers class * implements app.marginalia.LlmGenerator { *; }
# The search model (EmbeddingGemma) is called the same way: without these two lines the release build renames `embed`, the call
# from the core fails with "embedder threw", and search and indexing stop working whenever no compute node answers.
-keep interface app.marginalia.TextEmbedder { *; }
-keepclassmembers class * implements app.marginalia.TextEmbedder { *; }
-keep class com.google.ai.edge.litertlm.** { *; }

# Speaker separation: the native library builds these objects and calls the callback by name.
-keep class com.k2fsa.sherpa.onnx.** { *; }
-keepclassmembers class app.marginalia.Diarizer$* { *; }
