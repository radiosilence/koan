package cc.blit.koan.ui.pages

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.material3.OutlinedTextField
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import cc.blit.koan.engine.KoanRuntime
import cc.blit.koan.theme.KoanButton
import cc.blit.koan.theme.KoanButtonKind
import cc.blit.koan.theme.KoanRole
import cc.blit.koan.theme.KoanSpace
import cc.blit.koan.theme.KoanText
import cc.blit.koan.theme.KoanTone

/**
 * Server setup and sign-in, shown by the shell in place of the tabs while the
 * server status is signed out or refused.
 *
 * Placeholder: the phase 2 form, in the theme's text and button, over
 * `runtime.settings.signIn`. The real page (invites, pairing, the server's
 * extensions) replaces it.
 */
@Composable
fun SignInPage(runtime: KoanRuntime) {
    var url by rememberSaveable { mutableStateOf("") }
    var user by rememberSaveable { mutableStateOf("") }
    var password by rememberSaveable { mutableStateOf("") }
    val busy by runtime.settings.signingIn.collectAsStateWithLifecycle()
    val error by runtime.settings.error.collectAsStateWithLifecycle()
    Column(
        Modifier.fillMaxSize().safeDrawingPadding().padding(KoanSpace.page),
        verticalArrangement = Arrangement.spacedBy(KoanSpace.m),
    ) {
        KoanText("kōan", KoanRole.Display, KoanTone.Strong)
        KoanText("sign in to your music server", KoanRole.Body, KoanTone.Muted)
        OutlinedTextField(url, { url = it }, Modifier.fillMaxWidth(), label = { KoanText("server url", KoanRole.Meta) }, singleLine = true)
        OutlinedTextField(user, { user = it }, Modifier.fillMaxWidth(), label = { KoanText("username", KoanRole.Meta) }, singleLine = true)
        OutlinedTextField(password, { password = it }, Modifier.fillMaxWidth(), label = { KoanText("password", KoanRole.Meta) }, singleLine = true)
        KoanButton(
            if (busy) "signing in" else "sign in", { runtime.settings.signIn(url, user, password) },
            kind = KoanButtonKind.Prominent, enabled = !busy,
        )
        error?.let { KoanText(it, KoanRole.Meta, KoanTone.Bad, data = true, maxLines = 4) }
    }
}
