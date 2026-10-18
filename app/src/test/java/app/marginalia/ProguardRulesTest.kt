package app.marginalia

import org.junit.Assert.assertTrue
import org.junit.Test
import java.io.File

/**
 * The Rust core calls these Kotlin interfaces by method name through JNI. The release build shrinks and renames methods, so a
 * missing keep rule does not show in a debug build or in the unit tests: it shows as "embedder threw" in the installed app, and
 * search and indexing stop working whenever no compute node answers. This test fails when a rule is missing.
 */
class ProguardRulesTest {
    private val rules: String = listOf("proguard-rules.pro", "app/proguard-rules.pro").map(::File).first { it.isFile }.readText()

    private fun keeps(type: String) = Regex("""-keep\s+(class|interface)\s+${Regex.escape(type)}\b""").containsMatchIn(rules)

    private fun keepsImplementations(type: String) = Regex("""-keepclassmembers\s+class\s+\*\s+implements\s+${Regex.escape(type)}\b""").containsMatchIn(rules)

    @Test fun everythingTheCoreCallsByNameIsKept() {
        assertTrue("Native", keeps("app.marginalia.Native"))
        assertTrue("NativeListener", keeps("app.marginalia.NativeListener"))
        assertTrue("LlmGenerator", keeps("app.marginalia.LlmGenerator"))
        assertTrue("classes implementing LlmGenerator", keepsImplementations("app.marginalia.LlmGenerator"))
        assertTrue("TextEmbedder", keeps("app.marginalia.TextEmbedder"))
        assertTrue("classes implementing TextEmbedder", keepsImplementations("app.marginalia.TextEmbedder"))
    }
}
