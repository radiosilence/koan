"""A demo library for the website's screenshots: twelve records with
gradient sleeves and invented names, as silent 48 kHz FLAC (the rate most
outputs already run at, so a cued track does not switch the device).

    python3 library.py <directory>
"""
import pathlib
import random
import subprocess
import sys

L = pathlib.Path(sys.argv[1])
random.seed(7)
ALBUMS = [
 ("Marlow & Finch","Low Tide Arcade",2021,"0xf08a5d","0xb83b5e",["Shoreline Pinball","Coin Return","Neon Breakwater","Pier Lights","Undertow (Reprise)","High Score Lullaby","Token Economy","Last Ferry"]),
 ("Ione Vega","Paper Satellites",2019,"0x3ec1d3","0x1f4e79",["Orbit Hymn","Low Signal","Relay","Ground Station","Paper Moon Protocol","Telemetry"]),
 ("The Quiet Arms","Night Bus Hymnal",2023,"0x2d3047","0x93b7be",["Last Stop","Sodium","Route 38","Upper Deck","Request Stop"]),
 ("Odessa Kline","Glasshouse",2018,"0xa8e6cf","0x3d8361",["Fern","Condensation","Cuttings","Palm House","Greenroom"]),
 ("Halden","Northern Static",2020,"0x45474b","0xd8d9da",["Pylon","Grid","Interference","Long Wave"]),
 ("Mira Okafor","Saltwater Radio",2022,"0xfcbad3","0x6a2c70",["Tidal","Dial","Brine","Harbour Lights","Sea Fret","Call Sign"]),
 ("Lantern Room","Deep Field",2017,"0x0b1a3a","0x1c2b52",["Undertow","Abyssal","Hadal","Pressure"]),
 ("Juno Park","Velvet Underpass",2024,"0x5f0f40","0xfb8b24",["Sodium Lights","Overpass","Night Market","Ring Road","Underpass"]),
 ("Tamsin Rowe","Second Hand Sun",2017,"0xffde7d","0xf6416c",["Marigold","Charity Shop","Faded Polaroid","Sunday Best"]),
 ("Calder Quartet","Lantern Room",2016,"0x8d6e63","0x263238",["I. Lento","II. Scherzo","III. Adagio","IV. Finale"]),
 ("Aster & Bloom","Meridian",2015,"0x00b4d8","0x03045e",["Noon","Equator","Prime","Longitude","Zenith"]),
 ("Kasimir","Afterglow FM",2023,"0xff9a8b","0x4a1942",["Static Heart","Late Show","Request Line","Sign Off"]),
]
for artist, album, year, c0, c1, tracks in ALBUMS:
    d = L / artist / album; d.mkdir(parents=True, exist_ok=True)
    subprocess.run(["ffmpeg","-y","-loglevel","error","-f","lavfi","-i",
        f"gradients=s=600x600:c0={c0}:c1={c1}:x0=0:y0=0:x1=600:y1=600:nb_colors=2:speed=0,format=rgb24",
        "-frames:v","1",str(d/"cover.jpg")],check=True)
    for n, t in enumerate(tracks, 1):
        secs = random.randint(170, 390)
        subprocess.run(["ffmpeg","-y","-loglevel","error","-f","lavfi","-i","anullsrc=r=48000:cl=stereo","-t",str(secs),
            "-c:a","flac","-metadata",f"artist={artist}","-metadata",f"album_artist={artist}","-metadata",f"album={album}",
            "-metadata",f"title={t}","-metadata",f"track={n}","-metadata",f"date={year}",str(d/f"{n:02d} {t}.flac")],check=True)
print(f"library in {L}")
