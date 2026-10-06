"""Leave a scanned demo library as the website's screenshots want it: a record
cued part-way into a track, lyrics for that track, and some favourites.

    python3 seed.py <config dir> [album] [track] [playing]

With an album and track, that is what is cued; `playing` (1) has it resume
playing at launch, which is how a stand-in device shows something playing.
"""
import glob
import json
import os
import sqlite3
import sys
import time

cfg = sys.argv[1]
cue_album = sys.argv[2] if len(sys.argv) > 2 else "Low Tide Arcade"
cue_track = sys.argv[3] if len(sys.argv) > 3 else "Neon Breakwater"
playing_now = len(sys.argv) > 4 and sys.argv[4] == "1"
db = sqlite3.connect(glob.glob(os.path.join(cfg, "*.db"))[0])


def album(title):
    return db.execute(
        """SELECT t.id, f.path, t.title, a.name, t.track_number, t.duration_ms, al.title, al.id, a.id
           FROM tracks t JOIN local_files f ON f.track_id = t.id JOIN albums al ON al.id = t.album_id
           JOIN artists a ON a.id = al.artist_id WHERE al.title = ? ORDER BY t.track_number""",
        (title,),
    ).fetchall()


playing = album(cue_album)
items = [
    dict(path=p, title=t, artist=a, album_artist=a, album=al, year=None, codec="flac",
         track_number=n, disc=1, duration_ms=d, db_id=i)
    for i, p, t, a, n, d, al, _, _ in playing
]
cue = next(i for i in items if i["title"] == cue_track)
db.execute("INSERT OR REPLACE INTO playback_state (id, queue_json, updated_at) VALUES (1, ?, datetime('now'))",
           (json.dumps(items),))
db.execute("""INSERT OR REPLACE INTO playback_position
              (id, cursor_id, position_ms, was_playing, shuffle, repeat, updated_at)
              VALUES (1, ?, 83000, ?, 0, 'off', datetime('now'))""", (cue["path"], int(playing_now)))

lines = [
    (0, "Coins in the slot and the tide coming in"), (14, "Lights on the pier like a high score"),
    (29, "Every wave a replay"), (41, "We kept the change for the last ferry"),
    (56, "Salt on the glass of the cabinet"), (70, "Your name in three letters"),
    (83, "And the sea keeps playing"), (97, "Long after the arcade goes dark"),
    (112, "Neon on the breakwater"), (126, "Blinking out the time"),
    (140, "Insert another evening"), (155, "Continue, continue"),
]
lrc = "\n".join(f"[{s // 60:02d}:{s % 60:02d}.00]{text}" for s, text in lines)
db.execute("""INSERT OR REPLACE INTO lyrics_cache (track_id, source, synced, content, fetched_at)
              VALUES (?, 'lrclib', 1, ?, ?)""", (cue["db_id"], lrc, int(time.time())))

for title in ("Low Tide Arcade", "Paper Satellites", "Velvet Underpass", "Meridian"):
    rows = album(title)
    db.execute("INSERT OR IGNORE INTO favourite_albums (album_id) VALUES (?)", (rows[0][7],))
for title in ("Saltwater Radio", "Glasshouse"):
    db.execute("INSERT OR IGNORE INTO favourite_artists (artist_id) VALUES (?)", (album(title)[0][8],))
for title, n in (("Night Bus Hymnal", 2), ("Second Hand Sun", 1), ("Deep Field", 3), ("Afterglow FM", 2)):
    db.execute("INSERT OR IGNORE INTO favourites (track_id) VALUES (?)", (album(title)[n - 1][0],))
db.commit()
print(f"{len(items)} queued, cursor {cue['title']}; lyrics and favourites seeded")
