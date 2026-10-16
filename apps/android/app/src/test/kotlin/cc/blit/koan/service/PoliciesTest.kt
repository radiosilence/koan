package cc.blit.koan.service

import android.media.AudioManager
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class AudioFocusPolicyTest {
    private val policy = AudioFocusPolicy()

    @Test
    fun `permanent loss pauses and never comes back by itself`() {
        assertEquals(FocusAction.Pause, policy.onFocusChange(AudioManager.AUDIOFOCUS_LOSS, playing = true))
        assertEquals(FocusAction.None, policy.onFocusChange(AudioManager.AUDIOFOCUS_GAIN, playing = false))
    }

    @Test
    fun `a transient loss pauses and the gain resumes`() {
        assertEquals(FocusAction.PauseUntilGain, policy.onFocusChange(AudioManager.AUDIOFOCUS_LOSS_TRANSIENT, true))
        assertTrue(policy.pausedByFocus)
        assertEquals(FocusAction.Resume, policy.onFocusChange(AudioManager.AUDIOFOCUS_GAIN, false))
        assertFalse(policy.pausedByFocus)
    }

    @Test
    fun `a transient loss that allows ducking ducks, and the gain restores the level`() {
        assertEquals(FocusAction.Duck, policy.onFocusChange(AudioManager.AUDIOFOCUS_LOSS_TRANSIENT_CAN_DUCK, true))
        assertTrue(policy.ducked)
        // Not twice.
        assertEquals(FocusAction.None, policy.onFocusChange(AudioManager.AUDIOFOCUS_LOSS_TRANSIENT_CAN_DUCK, true))
        assertEquals(FocusAction.Unduck, policy.onFocusChange(AudioManager.AUDIOFOCUS_GAIN, true))
        assertFalse(policy.ducked)
    }

    @Test
    fun `a pause the user asked for is not undone by a gain`() {
        policy.onFocusChange(AudioManager.AUDIOFOCUS_LOSS_TRANSIENT, playing = true)
        policy.userPaused()
        assertEquals(FocusAction.None, policy.onFocusChange(AudioManager.AUDIOFOCUS_GAIN, false))
    }

    @Test
    fun `nothing playing means nothing to do on a loss`() {
        assertEquals(FocusAction.None, policy.onFocusChange(AudioManager.AUDIOFOCUS_LOSS_TRANSIENT, playing = false))
        assertEquals(FocusAction.None, policy.onFocusChange(AudioManager.AUDIOFOCUS_GAIN, false))
    }
}

class OutputRoutesTest {
    private val usb = OutputDevice(7, "DAC", RouteKind.Usb)
    private val speaker = OutputDevice(2, "Speaker", RouteKind.Speaker)
    private val bt = OutputDevice(5, "Buds", RouteKind.Bluetooth)
    private val wired = OutputDevice(4, "Headphones", RouteKind.Wired)

    @Test
    fun `a DAC beats wires beats Bluetooth beats the speaker`() {
        assertEquals(usb, OutputRoutes.choose(listOf(speaker, bt, wired, usb)))
        assertEquals(wired, OutputRoutes.choose(listOf(speaker, bt, wired)))
        assertEquals(bt, OutputRoutes.choose(listOf(speaker, bt)))
        assertEquals(speaker, OutputRoutes.choose(listOf(speaker)))
        assertNull(OutputRoutes.choose(emptyList()))
    }

    @Test
    fun `no device is the default output`() {
        val route = OutputRoutes.route(null, 48000)
        assertEquals(0, route.deviceId)
        assertEquals(48000, route.mixerRate)
        assertEquals(OutputRoute("DAC", 7, 96000, RouteKind.Usb), OutputRoutes.route(usb, 96000))
    }
}

class BitPerfectTest {
    private val usb = OutputRoute("DAC", 7, 48000, RouteKind.Usb)
    private val speaker = OutputRoute("Speaker", 2, 48000, RouteKind.Speaker)

    @Test
    fun `exclusive at the source rate on a DAC is bit-perfect`() {
        val grant = Grant(exclusive = true, streamRate = 96000, hardwareRate = 96000, deviceId = 7)
        assertEquals(BitPerfectStatus.Exclusive, BitPerfect.status(usb, grant, 96000, mixerBitPerfectSet = false))
    }

    @Test
    fun `a bit-perfect mixer at the source rate counts only if it was set`() {
        val grant = Grant(exclusive = false, streamRate = 44100, hardwareRate = 44100, deviceId = 7)
        assertEquals(BitPerfectStatus.MixerBitPerfect, BitPerfect.status(usb, grant, 44100, true))
        assertTrue(BitPerfect.status(usb, grant, 44100, false) is BitPerfectStatus.No)
    }

    @Test
    fun `resampled, or not a DAC, is never bit-perfect`() {
        val resampled = Grant(false, 44100, 48000, 7)
        val no = BitPerfect.status(usb, resampled, 44100, true) as BitPerfectStatus.No
        assertTrue(no.reason, no.reason.contains("48000"))
        val exclusive = Grant(true, 44100, 44100, 2)
        assertTrue(BitPerfect.status(speaker, exclusive, 44100, false) is BitPerfectStatus.No)
        assertTrue(BitPerfect.status(usb, null, 44100, false) is BitPerfectStatus.No)
        assertTrue(BitPerfect.status(usb, Grant(true, 48000, 48000, 7), 44100, false) is BitPerfectStatus.No)
        assertTrue(BitPerfect.status(usb, Grant(true, 44100, 44100, 7), 0, false) is BitPerfectStatus.No)
    }

    @Test
    fun `the mixer mode is the bit-perfect one at the source rate with the widest format`() {
        val modes = listOf(
            MixerMode(44100, bitPerfect = false, encodingRank = 5),
            MixerMode(44100, bitPerfect = true, encodingRank = 2),
            MixerMode(44100, bitPerfect = true, encodingRank = 4),
            MixerMode(96000, bitPerfect = true, encodingRank = 5),
        )
        assertEquals(MixerMode(44100, true, 4), BitPerfect.pickMixerMode(modes, 44100))
        assertNull(BitPerfect.pickMixerMode(modes, 192000))
    }
}
