// The share page's player. Tracks the browser can decode are decoded ahead
// and each is scheduled to start on the exact sample the last one ends, so
// albums that run into each other play without a gap. A track too long to
// hold decoded (a DJ mix) streams instead, where a gap at its end is moot.
(() => {
  const LONG = 900; // seconds
  const $ = (id) => document.getElementById(id);
  const rows = [...document.querySelectorAll("#tracks li")];
  const tracks = rows.map((li) => ({
    li,
    src: li.dataset.src,
    dur: Number(li.dataset.dur) || 0,
    title: li.dataset.title,
    artist: li.dataset.artist,
  }));
  const play = $("play"), prev = $("prev"), next = $("next");
  const seek = $("seek"), pos = $("pos"), len = $("len"), cover = $("cover");
  const album = document.querySelector("h1").textContent;
  cover.addEventListener("error", () => { cover.hidden = true; });

  let ctx = null;           // AudioContext, made on the first press
  let cur = -1;             // playing track
  let playing = false;
  let gen = 0;              // bumped on every jump, so stale callbacks do nothing
  let mode = null;          // "buffer" or "stream"
  let source = null, startedAt = 0;          // buffer mode
  let queued = null;        // { i, source, at } — the next track, scheduled gaplessly
  const buffers = new Map();                 // i -> Promise<AudioBuffer|null>
  const el = new Audio();                     // stream mode
  el.preload = "auto";

  const fmt = (s) => {
    s = Math.max(0, Math.floor(s || 0));
    return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
  };
  const decodable = (i) => tracks[i] && tracks[i].dur > 0 && tracks[i].dur <= LONG;

  // Must run inside the tap itself. iOS lets a page start audio only in the
  // gesture: a context created or resumed after an await stays silent. And
  // Web Audio obeys the ring/silent switch unless the page says it is a
  // media player, which an <audio> element never had to.
  let elUnlocked = false;
  function unlock(target) {
    if (navigator.audioSession) navigator.audioSession.type = "playback";
    if (!ctx && window.AudioContext) ctx = new AudioContext();
    if (ctx && ctx.state !== "running") ctx.resume();
    // The streaming element needs the same blessing: once it has started in
    // a gesture it may start again later, after a fetch or at a track change.
    if (!elUnlocked && tracks[target]) {
      elUnlocked = true;
      el.src = tracks[target].src;
      el.play().catch(() => {});
    }
  }

  function buffer(i) {
    if (!decodable(i)) return Promise.resolve(null);
    if (!buffers.has(i)) {
      buffers.set(i, fetch(tracks[i].src)
        .then((r) => (r.ok ? r.arrayBuffer() : Promise.reject(r.status)))
        .then((b) => ctx.decodeAudioData(b))
        .catch(() => null));
    }
    // Hold the current neighbourhood only: decoded audio is large.
    for (const k of buffers.keys()) if (k < i - 1 || k > i + 1) buffers.delete(k);
    return buffers.get(i);
  }

  function stop() {
    gen++;
    for (const s of [source, queued && queued.source]) {
      if (s) { s.onended = null; try { s.stop(); } catch {} }
    }
    source = null; queued = null;
    el.pause(); el.removeAttribute("src"); el.load();
  }

  function show(i) {
    rows.forEach((li, k) => li.classList.toggle("playing", k === i));
    const t = tracks[i];
    len.textContent = fmt(t.dur);
    seek.max = t.dur || 0;
    if ("mediaSession" in navigator) {
      navigator.mediaSession.metadata = new MediaMetadata({
        title: t.title, artist: t.artist, album,
        artwork: cover.hidden ? [] : [{ src: cover.src }],
      });
    }
  }

  // Start a decoded track at `from` seconds, `at` in context time.
  function startBuffer(i, buf, from, at) {
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
        cur = queued.i; source = queued.source; startedAt = queued.at; queued = null;
        source.onended = onBufferEnded(g);
        show(cur);
        queueNext(g);
      } else {
        go(cur + 1, 0);
      }
    };
  }

  async function queueNext(g) {
    const i = cur + 1;
    if (!decodable(i) || !source) return;
    const buf = await buffer(i);
    if (g !== gen || !buf || !source) return;
    const at = startedAt + source.buffer.duration;
    if (at < ctx.currentTime + 0.05) return; // too late to be seamless; onended will follow on
    queued = { i, source: startBuffer(i, buf, 0, at), at };
  }

  async function go(i, from) {
    if (i < 0 || i >= tracks.length) { stop(); cur = -1; setPlaying(false); rows.forEach((li) => li.classList.remove("playing")); return; }
    stop();
    const g = gen;
    cur = i;
    show(i);
    setPlaying(true);
    const buf = ctx ? await buffer(i) : null;
    if (g !== gen) return;
    if (buf) {
      mode = "buffer";
      if (ctx.state === "suspended") await ctx.resume();
      const at = ctx.currentTime + 0.03;
      source = startBuffer(i, buf, from, at);
      startedAt = at - from;
      source.onended = onBufferEnded(g);
      queueNext(g);
      buffer(i + 1);
    } else {
      mode = "stream";
      el.src = tracks[i].src;
      el.currentTime = from;
      el.onended = () => { if (g === gen) go(cur + 1, 0); };
      el.play().catch(() => setPlaying(false));
    }
  }

  function position() {
    if (cur < 0) return 0;
    return mode === "buffer" ? ctx.currentTime - startedAt : el.currentTime;
  }

  function setPlaying(p) {
    playing = p;
    play.textContent = p ? "Pause" : "Play";
    if ("mediaSession" in navigator) navigator.mediaSession.playbackState = p ? "playing" : "paused";
  }

  async function toggle() {
    unlock(cur < 0 ? 0 : cur);
    if (cur < 0) return go(0, 0);
    if (playing) {
      if (mode === "buffer") await ctx.suspend(); else el.pause();
      setPlaying(false);
    } else {
      if (mode === "buffer") await ctx.resume(); else await el.play();
      setPlaying(true);
    }
  }

  play.addEventListener("click", toggle);
  prev.addEventListener("click", () => { unlock(cur); go(position() > 3 ? cur : cur - 1, 0); });
  next.addEventListener("click", () => { unlock(cur); go(cur + 1, 0); });
  seek.addEventListener("change", () => { unlock(cur); if (cur >= 0) go(cur, Number(seek.value)); });
  rows.forEach((li, i) => {
    const pick = () => { unlock(i); go(i, 0); };
    li.addEventListener("click", pick);
    li.addEventListener("keydown", (e) => { if (e.key === "Enter" || e.key === " ") { e.preventDefault(); pick(); } });
  });
  if ("mediaSession" in navigator) {
    const ms = navigator.mediaSession;
    ms.setActionHandler("play", toggle);
    ms.setActionHandler("pause", toggle);
    ms.setActionHandler("previoustrack", () => go(cur - 1, 0));
    ms.setActionHandler("nexttrack", () => go(cur + 1, 0));
    ms.setActionHandler("seekto", (d) => go(cur, d.seekTime));
  }
  setInterval(() => {
    if (cur < 0) return;
    const p = position();
    pos.textContent = fmt(p);
    if (document.activeElement !== seek) seek.value = p;
  }, 250);
})();
