package cc.blit.koan.ui.settings

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.Button
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.navigation3.runtime.EntryProviderScope
import androidx.navigation3.runtime.NavKey
import cc.blit.koan.ui.StubPage
import cc.blit.koan.ui.navigation.Tab
import cc.blit.koan.ui.theme.LocalKoanTokens

fun EntryProviderScope<NavKey>.settingsEntries() {
    entry<Tab.Settings> { StubPage("Settings") }
}

/** Sign in to a koan or Subsonic server: what the app shows until one is signed in to. */
@Composable
fun SignInPage(model: AccountModel) {
    val state by model.state.collectAsStateWithLifecycle()
    var url by rememberSaveable { mutableStateOf("") }
    var username by rememberSaveable { mutableStateOf("") }
    var password by rememberSaveable { mutableStateOf("") }
    Box(Modifier.fillMaxSize().safeDrawingPadding()) {
        Column(
            Modifier.padding(LocalKoanTokens.current.gutter).fillMaxWidth(),
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            Text("kōan", style = MaterialTheme.typography.headlineMedium)
            OutlinedTextField(
                url,
                { url = it },
                label = { Text("Server") },
                singleLine = true,
                keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Uri),
                modifier = Modifier.fillMaxWidth(),
            )
            OutlinedTextField(username, { username = it }, label = { Text("User") }, singleLine = true, modifier = Modifier.fillMaxWidth())
            OutlinedTextField(
                password,
                { password = it },
                label = { Text("Password") },
                singleLine = true,
                visualTransformation = PasswordVisualTransformation(),
                keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Password),
                modifier = Modifier.fillMaxWidth(),
            )
            state.error?.let { Text(it, color = MaterialTheme.colorScheme.error) }
            Button(
                onClick = { model.signIn(url, username, password) },
                enabled = state.busy == null && url.isNotBlank() && username.isNotBlank(),
            ) { Text("Sign in") }
        }
        state.busy?.let { label ->
            Column(Modifier.align(Alignment.Center), horizontalAlignment = Alignment.CenterHorizontally) {
                CircularProgressIndicator()
                Text(label, Modifier.padding(top = 12.dp))
            }
        }
    }
}
