// The share page: the shared tracks as one queue on koan's gapless player,
// cued to the track the share starts on.
import type { Track } from "koan/player";
import { Player, clock, dragged, fill, fmt } from "koan/player";

const $ = (id: string) => document.getElementById(id);
const rows = [...document.querySelectorAll<HTMLElement>("#tracks li[data-src]")];
const start = Number($("tracks")?.dataset.start);
const play = $("play"), prev = $("prev"), next = $("next");
const seek = $("seek"), pos = $("pos"), len = $("len"), cover = $("cover");
if (!play || !prev || !next || !(seek instanceof HTMLInputElement) || !pos || !len || !cover) {
  throw new Error("the share page is missing its player controls");
}
cover.addEventListener("error", () => { cover.hidden = true; });
for (const img of document.querySelectorAll("img.art")) {
  img.addEventListener("error", () => img.classList.add("missing"));
}

const tracks = rows.map((li) => ({
  src: li.dataset.src ?? "",
  dur: Number(li.dataset.dur) || 0,
  title: li.dataset.title,
  artist: li.dataset.artist,
  album: li.dataset.album,
  cover: `${li.dataset.src}/cover`,
}));

const dragging = dragged("#seek");
seek.addEventListener("input", () => fill(seek));
const show = () => {
  const p = player.position;
  pos.textContent = fmt(p);
  if (dragging() !== seek) seek.value = String(p);
  fill(seek);
};
const ticking = clock(show);

const player = new Player<Track>({
  onChange({ cur, playing, track }) {
    rows.forEach((li, k) => li.classList.toggle("playing", k === cur));
    play.textContent = playing ? "Pause" : "Play";
    len.textContent = fmt(track ? track.dur : 0);
    seek.max = String(track ? track.dur : 0);
    show();
    ticking(playing);
  },
});
player.set(tracks, start >= 0 ? start : -1);

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
