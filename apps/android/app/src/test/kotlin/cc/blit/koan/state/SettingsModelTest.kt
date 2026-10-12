package cc.blit.koan.state

import org.junit.Assert.assertEquals
import org.junit.Test

class SettingsModelTest {
    @Test
    fun `no settings yet is loading`() {
        assertEquals(ServerStatus.Loading, SettingsModel.status(null, refused = false, linked = false))
    }
}
