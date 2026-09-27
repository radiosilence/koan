// koan's browser player: a play queue over the Web Audio clock. Tracks the
// browser can decode are decoded ahead and each is scheduled to start on the
// exact sample the last one ends, so albums that run into each other play
// without a gap. A track too long to hold decoded (a DJ mix), or one the
// browser cannot decode, streams through an <audio> element instead, where a
// gap at its end is moot. Used by the share page and the web UI.
//
// A track is { src, dur (seconds), title, artist, album, cover }; any other
// fields ride along untouched.
window.KoanPlayer = (opts = {}) => {
  const LONG = 900; // seconds
  const changed = opts.onChange || (() => {});
  // Called when a stream is refused, to renew the session before one retry.
  const renew = opts.renew || null;

  let queue = [];
  let cur = -1;             // playing (or paused) track
  let resumeAt = 0;         // where a restored queue resumes
  let playing = false;
  let gen = 0;              // bumped on every jump, so stale callbacks do nothing
  let mode = null;          // "buffer", "stream", or null when stopped
  let ctx = null;           // AudioContext, made on the first press
  let source = null, startedAt = 0;          // buffer mode
  let queued = null;        // { source, at }: the next track, scheduled gaplessly
  const buffers = new Map();                 // src -> Promise<AudioBuffer|null>
  const el = new Audio();                     // stream mode
  el.preload = "auto";

  const decodable = (t) => t && t.dur > 0 && t.dur <= LONG;

  // Runs synchronously at the top of every gesture handler, before any await:
  // iOS only lets a context start from inside the gesture itself.
  function wake() {
    // Without this iOS treats Web Audio as a sound effect, silenced by the mute switch.
    if (navigator.audioSession) { try { navigator.audioSession.type = "playback"; } catch {} }
    if (!ctx && window.AudioContext) ctx = new AudioContext();
    if (ctx && ctx.state !== "running") ctx.resume().catch(() => {});
  }

  function buffer(i) {
    const t = queue[i];
    if (!ctx || !decodable(t)) return Promise.resolve(null);
    if (!buffers.has(t.src)) {
      buffers.set(t.src, fetch(t.src, { credentials: "same-origin" })
        .then((r) => (r.ok ? r.arrayBuffer() : Promise.reject(r.status)))
        .then((b) => ctx.decodeAudioData(b))
        .catch(() => null));
    }
    // Hold the current neighbourhood only: decoded audio is large.
    const keep = new Set([cur - 1, cur, cur + 1, i].map((k) => queue[k] && queue[k].src));
    for (const k of buffers.keys()) if (!keep.has(k)) buffers.delete(k);
    return buffers.get(t.src);
  }

  function unqueue() {
    if (!queued) return;
    queued.source.onended = null;
    try { queued.source.stop(); } catch {}
    queued = null;
  }

  function stop() {
    gen++;
    if (source) { source.onended = null; try { source.stop(); } catch {} }
    source = null;
    unqueue();
    el.onended = el.onerror = null;
    el.pause(); el.removeAttribute("src"); el.load();
    mode = null;
  }

  function state() {
    return { queue, cur, playing, track: queue[cur] || null };
  }

  function session() {
    if (!("mediaSession" in navigator)) return;
    const ms = navigator.mediaSession;
    const t = queue[cur];
    ms.metadata = t ? new MediaMetadata({
      title: t.title || "", artist: t.artist || "", album: t.album || "",
      artwork: t.cover ? [{ src: new URL(t.cover, location.href).href }] : [],
    }) : null;
    ms.playbackState = t ? (playing ? "playing" : "paused") : "none";
    if (t && t.dur > 0 && ms.setPositionState) {
      try { ms.setPositionState({ duration: t.dur, position: Math.min(position(), t.dur), playbackRate: 1 }); } catch {}
    }
  }

  function notify() { session(); changed(state()); }

  function setPlaying(p) { playing = p; notify(); }

  function startBuffer(buf, from, at) {
    const s = ctx.createBufferSource();
    s.buffer = buf;
    s.connect(ctx.destination);
    s.start(at, from);
    return s;
  }

  function onBufferEnded(g) {
    return () => {
      if (g !== gen) return;
      if (queued) {
        // The next track already started on the last sample of this one.
        cur++;
        source = queued.source; startedAt = queued.at; queued = null;
        source.onended = onBufferEnded(g);
        notify();
        queueNext(g);
      } else {
        go(cur + 1, 0);
      }
    };
  }

  async function queueNext(g) {
    const i = cur + 1;
    if (mode !== "buffer" || !source || queued || !decodable(queue[i])) return;
    const buf = await buffer(i);
    if (g !== gen || !buf || !source || queued || cur + 1 !== i) return;
    const at = startedAt + source.buffer.duration;
    if (at < ctx.currentTime + 0.05) return; // too late to be seamless; onended will follow on
    queued = { source: startBuffer(buf, 0, at), at };
  }

  // The track after this one changed: drop what was scheduled for the old one.
  function requeue() {
    if (queued && queued.at <= ctx.currentTime) return; // already sounding
    unqueue();
    queueNext(gen);
  }

  async function go(i, from) {
    stop();
    if (i < 0 || i >= queue.length) { cur = -1; resumeAt = 0; setPlaying(false); return; }
    const g = gen;
    cur = i;
    resumeAt = 0;
    setPlaying(true);
    // A track that will stream starts without an await, so play() is still
    // inside the gesture; a decode that fails falls back to it afterwards.
    const buf = ctx && decodable(queue[i]) ? await buffer(i) : null;
    if (g !== gen) return;
    if (buf) {
      mode = "buffer";
      if (ctx.state !== "running") await ctx.resume();
      const at = ctx.currentTime + 0.03;
      source = startBuffer(buf, from, at);
      startedAt = at - from;
      source.onended = onBufferEnded(g);
      queueNext(g);
    } else {
      mode = "stream";
      const t = queue[i];
      let retried = false;
      el.src = t.src;
      el.currentTime = from;
      el.onended = () => { if (g === gen) go(cur + 1, 0); };
      el.onerror = async () => {
        if (g !== gen) return;
        if (!retried && renew && await renew() && g === gen) {
          retried = true;
          const at = el.currentTime;
          el.src = t.src;
          el.currentTime = at;
          el.play().catch(() => {});
          return;
        }
        go(cur + 1, 0);
      };
      el.play().catch(() => { if (g === gen) setPlaying(false); });
    }
    buffer(i + 1);
    session();
  }

  function position() {
    if (cur < 0) return 0;
    if (mode === "buffer") return Math.max(0, ctx.currentTime - startedAt);
    if (mode === "stream") return el.currentTime;
    return resumeAt;
  }

  async function toggle() {
    wake();
    if (!queue.length) return;
    if (mode === null) return go(Math.max(cur, 0), resumeAt);
    if (playing) {
      if (mode === "buffer") await ctx.suspend(); else el.pause();
      setPlaying(false);
    } else {
      if (mode === "buffer") await ctx.resume(); else await el.play().catch(() => {});
      setPlaying(true);
    }
  }

  if ("mediaSession" in navigator) {
    const ms = navigator.mediaSession;
    const on = (action, fn) => { try { ms.setActionHandler(action, fn); } catch {} };
    on("play", () => { if (!playing) toggle(); });
    on("pause", () => { if (playing) toggle(); });
    on("previoustrack", () => api.prev());
    on("nexttrack", () => api.next());
    on("seekto", (d) => api.seek(d.seekTime));
  }

  const api = {
    state,
    position,
    toggle,
    // Load a queue without playing it: a restored session, or the share page.
    set(tracks, i = -1, from = 0) {
      stop();
      queue = tracks.slice();
      cur = i < queue.length ? i : -1;
      resumeAt = cur >= 0 ? from : 0;
      setPlaying(false);
    },
    // Replace the queue and play from track i.
    play(tracks, i = 0) { wake(); queue = tracks.slice(); go(i, 0); },
    // Play these next, starting now, and keep the rest of the queue after them.
    playNow(tracks) { wake(); queue.splice(cur + 1, 0, ...tracks); go(cur + 1, 0); },
    append(tracks) {
      const wasLast = cur === queue.length - 1;
      queue.push(...tracks);
      if (wasLast && mode === "buffer") requeue();
      notify();
    },
    playAt(i, from = 0) { wake(); go(i, from); },
    remove(i) {
      if (i < 0 || i >= queue.length) return;
      queue.splice(i, 1);
      if (i === cur) { if (playing) go(i, 0); else { stop(); cur = i < queue.length ? i : -1; resumeAt = 0; notify(); } return; }
      if (i < cur) cur--;
      else if (i === cur + 1 && mode === "buffer") requeue();
      notify();
    },
    clear() { stop(); queue = []; cur = -1; resumeAt = 0; setPlaying(false); },
    next() { wake(); if (cur + 1 < queue.length) go(cur + 1, 0); },
    prev() { wake(); go(position() > 3 || cur <= 0 ? Math.max(cur, 0) : cur - 1, 0); },
    seek(s) {
      if (cur < 0) return;
      wake();
      if (mode === null) { resumeAt = s; notify(); return; }
      go(cur, s);
    },
  };
  return api;
};
