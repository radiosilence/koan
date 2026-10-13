package cc.blit.koan.ui.theme

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge

/**
 * The theme sheet on its own, in debug builds only:
 * `adb shell am start -n cc.blit.koan/.ui.theme.ThemeSheetActivity`.
 */
class ThemeSheetActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        setContent { ThemeSheet() }
    }
}
