package cc.blit.koan.state

import uniffi.koan_ffi.Appearance
import uniffi.koan_ffi.Invite
import uniffi.koan_ffi.LibraryFolder
import uniffi.koan_ffi.Settings

/** A signed-in configuration with every field set, for a test to vary with `copy`. */
fun settingsRecord(
    signedIn: Boolean = true,
    url: String = "https://music.example",
    user: String = "ada",
) = Settings(
    libraryFolders = listOf(LibraryFolder(path = "/sdcard/Music", tracks = 12u)),
    remoteEnabled = signedIn,
    remoteUrl = if (signedIn) url else "",
    remoteUsername = if (signedIn) user else "",
    remoteSignedIn = signedIn,
    remoteTracks = 100u,
    downloadWorkers = 5u,
    cacheLimit = "",
    cacheDir = "/cache",
    cacheBytes = 0u,
    autoSync = true,
    autoSyncIntervalMins = 60u,
    playQueue = false,
    replaygain = "off",
    preAmpDb = 0.0,
    fadeOnPause = false,
    devicesDiscoverable = false,
    devicesAddresses = emptyList(),
    devicesRefused = emptyList(),
    devicesNearbyControl = "full",
    devicesKeepRunning = false,
)

fun appearanceRecord(recordColours: Boolean = true) = Appearance(
    koan = true, icons = true, recordColours = recordColours, washWindow = true, rainbow = false, textSize = 100u,
)

fun inviteRecord(server: String = "https://music.example", user: String = "ada") = Invite(
    server = server, username = user, token = "tok", password = null, link = "https://koan.rocks/join/#x",
    emailSubject = "", emailText = "", emailHtml = "", mailto = "",
)
