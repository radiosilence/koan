package cc.blit.koan.state

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class SettingsRulesTest {
    @Test
    fun `a cache limit is blank, or a positive size with an optional unit`() {
        assertEquals("", SettingsRules.cacheLimit(""))
        assertEquals("", SettingsRules.cacheLimit("   "))
        assertEquals("50GB", SettingsRules.cacheLimit("50gb"))
        assertEquals("1.5TB", SettingsRules.cacheLimit(" 1.5 tb "))
        assertEquals("500M", SettingsRules.cacheLimit("500m"))
        assertEquals("1024", SettingsRules.cacheLimit("1024"))
    }

    @Test
    fun `a cache limit that is not a size is refused`() {
        for (bad in listOf("lots", "GB", "-5GB", "0", "0GB", "5 XB", "5GB extra", "1.2.3GB")) {
            assertNull("'$bad' should be refused", SettingsRules.cacheLimit(bad))
        }
    }

    @Test
    fun `an address is a host with an optional port in range`() {
        for (ok in listOf("nas", "nas.local", "10.0.0.5", "nas.local:5626", "my-host_1:80", "[::1]", "[fe80::1]:5626", "fe80::1")) {
            assertEquals(ok, SettingsRules.address(ok))
        }
        assertEquals("nas:5626", SettingsRules.address("  nas:5626 "))
    }

    @Test
    fun `an address that is a url, a path, or out of range is refused`() {
        for (bad in listOf("", "  ", "http://nas", "nas/path", "user@nas", "nas:0", "nas:65536", "nas:x", "a b", ":5626", "[::1", "[::1]x", "[]:80", "[nas]:80")) {
            assertNull("'$bad' should be refused", SettingsRules.address(bad))
        }
    }

    @Test
    fun `workers and pre-amp stay in the engine's range`() {
        assertEquals(1u, SettingsRules.workers(0))
        assertEquals(16u, SettingsRules.workers(99))
        assertEquals(7u, SettingsRules.workers(7))
        assertEquals(15.0, SettingsRules.preAmp(40.0), 0.0)
        assertEquals(-15.0, SettingsRules.preAmp(-40.0), 0.0)
        assertEquals(1.5, SettingsRules.preAmp(1.4), 0.0)
        assertEquals("-3.5 db", SettingsRules.preAmpText(-3.5))
    }

    @Test
    fun `bytes print in binary multiples`() {
        assertEquals("0 B", SettingsRules.bytes(0))
        assertEquals("1.0 KB", SettingsRules.bytes(1024))
        assertEquals("1.5 GB", SettingsRules.bytes(1_610_612_736))
        assertEquals("250 MB", SettingsRules.bytes(262_144_000))
    }

    @Test
    fun `a pairing code is forgiven its case, spaces and dash`() {
        assertEquals("ABCD-EF12", PairCode.normalise("abcd-ef12"))
        assertEquals("ABCD-EF12", PairCode.normalise(" abcd ef12 "))
        assertEquals("ABCD-EF12", PairCode.normalise("ABCDEF12"))
        assertNull(PairCode.normalise("ABCD"))
        assertNull(PairCode.normalise("ABCD-EF12-3"))
        assertNull(PairCode.normalise(""))
    }

    @Test
    fun `the host of a server is shown, the address when it will not parse`() {
        assertEquals("music.example", SettingsModel.hostOf("https://music.example:4533/"))
        assertEquals("not a url", SettingsModel.hostOf("not a url"))
    }
}
