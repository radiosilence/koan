// The share page: the shared tracks as one queue on koan's gapless player.
(() => {
  const $ = (id) => document.getElementById(id);
  const rows = [...document.querySelectorAll("#tracks li")];
  const play = $("play"), prev = $("prev"), next = $("next");
  const seek = $("seek"), pos = $("pos"), len = $("len"), cover = $("cover");
  const album = document.querySelector("h1").textContent;
  let hasCover = true;
  cover.addEventListener("error", () => { cover.hidden = true; hasCover = false; });

  const tracks = rows.map((li) => ({
    src: li.dataset.src,
    dur: Number(li.dataset.dur) || 0,
    title: li.dataset.title,
    artist: li.dataset.artist,
    album,
    get cover() { return hasCover ? cover.src : null; },
  }));

  const fmt = (s) => {
    s = Math.max(0, Math.floor(s || 0));
    return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
  };

  // Set while the range is dragged; focus stays on a clicked range, so it is
  // no guide to whether the clock may move it.
  let dragging = false;
  for (const type of ["pointerdown", "touchstart"]) seek.addEventListener(type, () => { dragging = true; }, { passive: true });
  for (const type of ["pointerup", "pointercancel", "touchend", "touchcancel", "change", "blur"]) {
    seek.addEventListener(type, () => { dragging = false; }, { passive: true });
  }
  const show = () => {
    const p = player.position();
    pos.textContent = fmt(p);
    if (!dragging) seek.value = p;
  };

  const player = window.KoanPlayer({
    onChange({ cur, playing, track }) {
      rows.forEach((li, k) => li.classList.toggle("playing", k === cur));
      play.textContent = playing ? "Pause" : "Play";
      len.textContent = fmt(track ? track.dur : 0);
      seek.max = track ? track.dur : 0;
      show();
    },
  });
  player.set(tracks);

  play.addEventListener("click", () => player.toggle());
  prev.addEventListener("click", () => player.prev());
  next.addEventListener("click", () => player.next());
  seek.addEventListener("change", () => player.seek(Number(seek.value)));
  rows.forEach((li, i) => {
    li.addEventListener("click", () => player.playAt(i));
    li.addEventListener("keydown", (e) => {
      if (e.key === "Enter" || e.key === " ") { e.preventDefault(); player.playAt(i); }
    });
  });
  setInterval(() => {
    if (player.state().cur >= 0) show();
  }, 250);
})();
