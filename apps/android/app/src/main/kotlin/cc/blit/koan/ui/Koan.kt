package cc.blit.koan.ui

import androidx.compose.runtime.Composable
import androidx.compose.ui.platform.LocalContext
import cc.blit.koan.KoanApp

/** The application, for its `mirror` and `player`, from inside a composable. */
@Composable
fun koanApp() = LocalContext.current.applicationContext as KoanApp
