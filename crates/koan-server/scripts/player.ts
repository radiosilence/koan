// koan's browser player: a play queue over the Web Audio clock. Tracks the
// browser can decode are decoded ahead and each is scheduled to start on the
// exact sample the last one ends, so albums that run into each other play
// without a gap. A track too long to hold decoded (a DJ mix), or one the
// browser cannot decode, streams through an <audio> element instead, where a
// gap at its end is moot. Used by the share page and the web UI.
//
// Decoded audio is scheduled on the AudioContext clock but heard through a
// media element (a MediaStream destination played by <audio>): iOS mutes Web
// Audio with the ring/silent switch and stops it in the background, and does
// neither to a media element. Only WebKit gives a MediaStream element a media
// session, though, and without one media keys and the system's controls never
// reach the page. Elsewhere the graph plays to the context's own destination
// and the same element loops near-silence beside it, to hold the session.
//
// A track is { src, dur (seconds), title, artist, album, cover }; any other
// fields ride along untouched.
//
// Both pages also get the tooltips at the end of this file.
export type Track = {
  src: string;
  dur: number;
  title?: string | undefined;
  artist?: string | undefined;
  album?: string | undefined;
  cover?: string | null | undefined;
};

type Options<T extends Track> = {
  onChange?: (player: Player<T>) => void;
  // Called when a stream is refused, to renew the session before one retry.
  renew?: () => Promise<boolean>;
};

const LONG = 900; // seconds

const decodable = (t: Track | undefined) => t && t.dur > 0 && t.dur <= LONG;

const webkit = !!navigator.audioSession;

// Six seconds, since Chrome treats anything under five as a sound effect
// rather than media, and a small constant offset rather than zero: Firefox
// gives an element a media session only once it hears something, and an
// offset is inaudible, survives resampling and loops without a seam.
let hushUrl: string | null = null;
function hush() {
  if (hushUrl) return hushUrl;
  const rate = 8000, n = rate * 6, v = new DataView(new ArrayBuffer(44 + n * 2));
  const ascii = (at: number, s: string) => { for (let i = 0; i < s.length; i++) v.setUint8(at + i, s.charCodeAt(i)); };
  ascii(0, "RIFF"); v.setUint32(4, 36 + n * 2, true); ascii(8, "WAVEfmt ");
  v.setUint32(16, 16, true); v.setUint16(20, 1, true); v.setUint16(22, 1, true);
  v.setUint32(24, rate, true); v.setUint32(28, rate * 2, true); v.setUint16(32, 2, true); v.setUint16(34, 16, true);
  ascii(36, "data"); v.setUint32(40, n * 2, true);
  for (let i = 0; i < n; i++) v.setInt16(44 + i * 2, 8, true);
  return hushUrl = URL.createObjectURL(new Blob([v.buffer], { type: "audio/wav" }));
}

export class Player<T extends Track> {
  #changed: (player: Player<T>) => void;
  #renew: (() => Promise<boolean>) | null;

  #queue: T[] = [];
  #cur = -1;                // playing (or paused) track
  #resumeAt = 0;            // where a restored queue resumes
  #playing = false;
  #gen = 0;                 // bumped on every jump, so stale callbacks do nothing
  // Decoded audio on the context's clock, a stream, or null when stopped.
  #mode: "buffer" | "stream" | null = null;
  #ctx: AudioContext | null = null; // made on the first press; always there in buffer mode
  #source: AudioBufferSourceNode | null = null; // buffer mode
  #startedAt = 0;
  #queued: { source: AudioBufferSourceNode; at: number } | null = null; // the next track, scheduled gaplessly
  #buffers = new Map<string, Promise<AudioBuffer | null>>(); // by src
  #el = new Audio();        // stream mode
  #dest: MediaStreamAudioDestinationNode | null = null; // MediaStream the graph plays into
  #out = new Audio();       // what is heard in buffer mode, or what holds its media session
  #elUnlocked = false;

  constructor({ onChange, renew }: Options<T> = {}) {
    this.#changed = onChange ?? (() => {});
    this.#renew = renew ?? null;
    this.#el.preload = "auto";
    this.#out.setAttribute("playsinline", "");
    // Paused or played from outside (lock screen, headphones, another app's
    // audio): follow it, so the clock and what is heard agree.
    this.#out.addEventListener("pause", () => {
      if (this.#playing && this.#mode === "buffer") { this.#playing = false; this.#ctx?.suspend(); this.#notify(); }
    });
    this.#out.addEventListener("play", () => {
      if (!this.#playing && this.#mode === "buffer") { this.#playing = true; this.#ctx?.resume(); this.#notify(); }
    });

    if ("mediaSession" in navigator) {
      const ms = navigator.mediaSession;
      const on = (action: MediaSessionAction, fn: MediaSessionActionHandler) => { try { ms.setActionHandler(action, fn); } catch {} };
      on("play", () => { if (!this.#playing) this.toggle(); });
      on("pause", () => { if (this.#playing) this.toggle(); });
      on("previoustrack", () => this.prev());
      on("nexttrack", () => this.next());
      on("seekto", (d) => { if (d.seekTime != null) this.seek(d.seekTime); });
    }
  }

  get queue() { return this.#queue; }
  get cur() { return this.#cur; }
  get playing() { return this.#playing; }
  get track() { return this.#queue[this.#cur] || null; }

  get position() {
    if (this.#cur < 0) return 0;
    if (this.#mode === "buffer" && this.#ctx) return Math.max(0, this.#ctx.currentTime - this.#startedAt);
    if (this.#mode === "stream") return this.#el.currentTime;
    return this.#resumeAt;
  }

  // Runs synchronously at the top of every gesture handler, before any await:
  // iOS only lets a context start from inside the gesture itself.
  #wake(t: T | null | undefined) {
    // Without this iOS treats Web Audio as a sound effect, silenced by the mute switch.
    if (navigator.audioSession) { try { navigator.audioSession.type = "playback"; } catch {} }
    if (!this.#ctx && window.AudioContext) this.#ctx = new AudioContext();
    const ctx = this.#ctx;
    if (ctx && webkit && !this.#dest && ctx.createMediaStreamDestination) {
      try { this.#dest = ctx.createMediaStreamDestination(); this.#out.srcObject = this.#dest.stream; } catch { this.#dest = null; }
    }
    if (!this.#dest && !this.#out.src) { this.#out.src = hush(); this.#out.loop = true; }
    if (ctx && ctx.state !== "running") ctx.resume().catch(() => {});
    // Started inside the tap, like the context, so iOS lets it run from here on.
    if (t) this.#hear();
    // The streaming element needs the same blessing: once started in a gesture
    // it may start again later, after a fetch or at a track change.
    if (!this.#elUnlocked && t) {
      this.#elUnlocked = true;
      this.#el.src = t.src;
      // Silenced again unless this tap is what starts it streaming.
      this.#el.play().then(() => { if (this.#mode !== "stream") this.#el.pause(); }).catch(() => {});
    }
  }

  #hear() { if (this.#dest || this.#out.src) this.#out.play().catch(() => {}); }

  #buffer(i: number) {
    const t = this.#queue[i], ctx = this.#ctx;
    if (!ctx || !t || !decodable(t)) return Promise.resolve(null);
    if (!this.#buffers.has(t.src)) {
      this.#buffers.set(t.src, fetch(t.src, { credentials: "same-origin" })
        .then((r) => (r.ok ? r.arrayBuffer() : Promise.reject(r.status)))
        .then((b) => ctx.decodeAudioData(b))
        .catch(() => null));
    }
    // Hold the current neighbourhood only: decoded audio is large.
    const cur = this.#cur;
    const keep = new Set([cur - 1, cur, cur + 1, i].map((k) => this.#queue[k]?.src));
    for (const k of this.#buffers.keys()) if (!keep.has(k)) this.#buffers.delete(k);
    return this.#buffers.get(t.src);
  }

  #unqueue() {
    if (!this.#queued) return;
    this.#queued.source.onended = null;
    try { this.#queued.source.stop(); } catch {}
    this.#queued = null;
  }

  #stop() {
    this.#gen++;
    if (this.#source) { this.#source.onended = null; try { this.#source.stop(); } catch {} }
    this.#source = null;
    this.#unqueue();
    this.#el.onended = this.#el.onerror = null;
    this.#el.pause(); this.#el.removeAttribute("src"); this.#el.load();
    this.#mode = null;
  }

  #session() {
    if (!("mediaSession" in navigator)) return;
    const ms = navigator.mediaSession;
    const t = this.#queue[this.#cur];
    ms.metadata = t ? new MediaMetadata({
      title: t.title || "", artist: t.artist || "", album: t.album || "",
      artwork: t.cover ? [{ src: new URL(t.cover, location.href).href }] : [],
    }) : null;
    ms.playbackState = t ? (this.#playing ? "playing" : "paused") : "none";
    if (t && t.dur > 0 && ms.setPositionState) {
      try { ms.setPositionState({ duration: t.dur, position: Math.min(this.position, t.dur), playbackRate: 1 }); } catch {}
    }
  }

  #notify() { this.#session(); this.#changed(this); }

  #setPlaying(p: boolean) { this.#playing = p; this.#notify(); }

  #startBuffer(ctx: AudioContext, buf: AudioBuffer, from: number, at: number) {
    const s = ctx.createBufferSource();
    s.buffer = buf;
    s.connect(this.#dest || ctx.destination);
    s.start(at, from);
    return s;
  }

  #ended(g: number) {
    if (g !== this.#gen) return;
    if (this.#queued) {
      // The next track already started on the last sample of this one.
      this.#cur++;
      const { source, at } = this.#queued;
      this.#source = source; this.#startedAt = at; this.#queued = null;
      source.onended = () => this.#ended(g);
      this.#notify();
      this.#queueNext(g);
    } else {
      this.#go(this.#cur + 1, 0);
    }
  }

  async #queueNext(g: number) {
    const i = this.#cur + 1;
    const ctx = this.#ctx;
    if (this.#mode !== "buffer" || !ctx || !this.#source || this.#queued || !decodable(this.#queue[i])) return;
    const buf = await this.#buffer(i);
    if (g !== this.#gen || !buf || !this.#source?.buffer || this.#queued || this.#cur + 1 !== i) return;
    const at = this.#startedAt + this.#source.buffer.duration;
    if (at < ctx.currentTime + 0.05) return; // too late to be seamless; onended will follow on
    this.#queued = { source: this.#startBuffer(ctx, buf, 0, at), at };
  }

  // The track after this one changed: drop what was scheduled for the old one.
  #requeue() {
    if (this.#queued && this.#ctx && this.#queued.at <= this.#ctx.currentTime) return; // already sounding
    this.#unqueue();
    this.#queueNext(this.#gen);
  }

  async #go(i: number, from: number) {
    this.#stop();
    if (i < 0 || i >= this.#queue.length) { this.#cur = -1; this.#resumeAt = 0; this.#setPlaying(false); this.#out.pause(); return; }
    const g = this.#gen;
    this.#cur = i;
    this.#resumeAt = 0;
    this.#setPlaying(true);
    // A track that will stream starts without an await, so play() is still
    // inside the gesture; a decode that fails falls back to it afterwards.
    const ctx = this.#ctx;
    const buf = ctx && decodable(this.#queue[i]) ? await this.#buffer(i) : null;
    if (g !== this.#gen) return;
    if (ctx && buf) {
      this.#mode = "buffer";
      this.#hear();
      if (ctx.state !== "running") await ctx.resume();
      const at = ctx.currentTime + 0.03;
      const source = this.#startBuffer(ctx, buf, from, at);
      this.#source = source;
      this.#startedAt = at - from;
      source.onended = () => this.#ended(g);
      this.#queueNext(g);
    } else {
      this.#mode = "stream";
      this.#out.pause();
      const el = this.#el, t = this.#queue[i];
      let retried = false;
      el.src = t.src;
      el.currentTime = from;
      el.onended = () => { if (g === this.#gen) this.#go(this.#cur + 1, 0); };
      el.onerror = async () => {
        if (g !== this.#gen) return;
        if (!retried && this.#renew && await this.#renew() && g === this.#gen) {
          retried = true;
          const at = el.currentTime;
          el.src = t.src;
          el.currentTime = at;
          el.play().catch(() => {});
          return;
        }
        this.#go(this.#cur + 1, 0);
      };
      el.play().catch(() => { if (g === this.#gen) this.#setPlaying(false); });
    }
    this.#buffer(i + 1);
    this.#session();
  }

  async toggle() {
    this.#wake(this.#playing ? null : this.#queue[Math.max(this.#cur, 0)]);
    if (!this.#queue.length) return;
    if (this.#mode === null) return this.#go(Math.max(this.#cur, 0), this.#resumeAt);
    // `playing` flips first, so the output element's own events see it and
    // leave it alone.
    if (this.#playing) {
      this.#playing = false;
      if (this.#mode === "buffer") { this.#out.pause(); await this.#ctx?.suspend(); } else this.#el.pause();
    } else {
      this.#playing = true;
      if (this.#mode === "buffer") { this.#hear(); await this.#ctx?.resume(); } else await this.#el.play().catch(() => {});
    }
    this.#notify();
  }

  // Load a queue without playing it: a restored session, or the share page.
  set(tracks: T[], i = -1, from = 0) {
    this.#stop();
    this.#queue = tracks.slice();
    this.#cur = i < this.#queue.length ? i : -1;
    this.#resumeAt = this.#cur >= 0 ? from : 0;
    this.#setPlaying(false);
  }

  // Replace the queue and play from track i.
  play(tracks: T[], i = 0) { this.#wake(tracks[i]); this.#queue = tracks.slice(); this.#go(i, 0); }

  // Play these next, starting now, and keep the rest of the queue after them.
  playNow(tracks: T[]) { this.#wake(tracks[0]); this.#queue.splice(this.#cur + 1, 0, ...tracks); this.#go(this.#cur + 1, 0); }

  // Queue these after the playing track, without interrupting it.
  playNext(tracks: T[]) {
    this.#queue.splice(this.#cur + 1, 0, ...tracks);
    if (this.#mode === "buffer") this.#requeue();
    this.#notify();
  }

  append(tracks: T[]) {
    const wasLast = this.#cur === this.#queue.length - 1;
    this.#queue.push(...tracks);
    if (wasLast && this.#mode === "buffer") this.#requeue();
    this.#notify();
  }

  playAt(i: number, from = 0) { this.#wake(this.#queue[i]); this.#go(i, from); }

  remove(i: number) {
    if (i < 0 || i >= this.#queue.length) return;
    this.#queue.splice(i, 1);
    if (i === this.#cur) {
      if (this.#playing) this.#go(i, 0);
      else { this.#stop(); this.#cur = i < this.#queue.length ? i : -1; this.#resumeAt = 0; this.#notify(); }
      return;
    }
    if (i < this.#cur) this.#cur--;
    else if (i === this.#cur + 1 && this.#mode === "buffer") this.#requeue();
    this.#notify();
  }

  clear() { this.#stop(); this.#queue = []; this.#cur = -1; this.#resumeAt = 0; this.#setPlaying(false); }

  next() { this.#wake(this.#queue[this.#cur + 1]); if (this.#cur + 1 < this.#queue.length) this.#go(this.#cur + 1, 0); }

  prev() {
    this.#wake(this.#queue[Math.max(this.#cur - 1, 0)]);
    this.#go(this.position > 3 || this.#cur <= 0 ? Math.max(this.#cur, 0) : this.#cur - 1, 0);
  }

  seek(s: number) {
    if (this.#cur < 0) return;
    this.#wake(this.#queue[this.#cur]);
    if (this.#mode === null) { this.#resumeAt = s; this.#notify(); return; }
    this.#go(this.#cur, s);
  }
}

// Text cut short by an ellipsis has the whole of it as its tooltip, on every
// page, with nothing in the markup asking for it. Decided as the pointer
// arrives, so it follows the width the text has now, and never put over a
// title the page gave the element itself: a title is ours only while it is the
// text we put there.
document.addEventListener("pointerover", (e) => {
  for (let el = e.target; el instanceof Element && el !== document.body; el = el.parentElement) {
    if (!(el instanceof HTMLElement) || getComputedStyle(el).textOverflow !== "ellipsis") continue;
    const ours = el.hasAttribute("data-cut") && el.title === el.dataset.cut;
    if (el.title && !ours) continue;
    if (el.scrollWidth > el.clientWidth) {
      el.title = el.dataset.cut = el.innerText.trim();
      return;
    }
    if (ours) el.removeAttribute("title");
  }
}, { passive: true });
