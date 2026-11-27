// The web UI's player: the transport and Now Playing, the queue it keeps in
// the browser, the track menu, and keeping the session alive. Pages load in
// place through Datastar and the kit's navigation, so this script never swaps
// markup: it reads tracks from the rows the server drew, every address among
// them written by the server, and asks the server for the queue's rows.
import type { Track } from "koan/player";
import { Player, clock, dragged, fill, fmt } from "koan/player";
import * as z from "zod/mini";

// A credit as the server writes it for the player (`parts::credit::for_player`):
// each part's text and, for a credited artist, their page.
const Credit = z.array(z.object({ text: z.string(), href: z.string() }));
type Credit = z.infer<typeof Credit>;

type UiTrack = Track & {
  id: number;
  albumHref: string;
  properties: string;
  credit: Credit;
};

// The queue as stored, written when its tracks change; `start` is where it
// begins in the queue played, when only the rest from the playing track
// fitted. A queue an older page stored lacks the addresses; its tracks keep
// what they have.
const Stored = z.object({
  id: z.number(),
  src: z.string(),
  dur: z.number(),
  title: z.optional(z.nullable(z.string())),
  artist: z.optional(z.nullable(z.string())),
  album: z.optional(z.nullable(z.string())),
  cover: z.optional(z.nullable(z.string())),
  albumHref: z.optional(z.string()),
  properties: z.optional(z.string()),
  credit: z.optional(z.nullable(Credit)),
});
const Saved = z.object({ queue: z.array(z.unknown()), start: z.optional(z.number()), cur: z.optional(z.number()), pos: z.optional(z.number()) });
// Where in the queue, written as the music plays.
const Place = z.object({ cur: z.number(), pos: z.number() });

const body = document.body;
document.documentElement.classList.add("js");
// The server groups history by day, and only the browser knows whose day.
document.cookie = `koan_tz=${-new Date().getTimezoneOffset()}; path=/; max-age=31536000; samesite=lax`;
const all = <E extends Element = HTMLElement>(sel: string, root: ParentNode = document) => [...root.querySelectorAll<E>(sel)];
const main = () => document.getElementById("main") ?? body;

// --- Session ---------------------------------------------------------------
// The access cookie lasts minutes. Renewing spends the refresh cookie, which
// only /auth routes receive, for fresh ones; no token is ever visible here.
// Behind an authenticating proxy there is no refresh cookie, so the page asks
// the proxy's route first. The page names both.
const rawFetch = window.fetch.bind(window);
let renewing: Promise<boolean> | null = null;
let renewedAt = Date.now();
// Renewed this long after the last renewal, whatever the page is doing, so a
// queue left playing still has a session to stream with.
const RENEW_EVERY = 10 * 60 * 1000;
let renewal = setTimeout(() => renew(), RENEW_EVERY);
function renewed() {
  renewedAt = Date.now();
  clearTimeout(renewal);
  renewal = setTimeout(() => renew(), RENEW_EVERY);
}
const post = (path: string) =>
  rawFetch(path, { method: "POST", credentials: "same-origin" }).then((r) => r.ok);
const first = body.dataset["renew"] ?? "";
const then = body.dataset["renewThen"] ?? "";
function renew() {
  renewing ??= post(first)
    .then((ok) => ok || post(then))
    .then((ok) => { if (ok) renewed(); return ok; })
    .catch(() => false)
    .finally(() => { renewing = null; });
  return renewing;
}
const sameOrigin = (input: RequestInfo | URL) =>
  new URL(input instanceof Request ? input.url : input, location.href).origin === location.origin;
// Every request this page makes, Datastar's included, renews once on a 401
// and retries; a session that cannot be renewed reloads into the sign-in form.
// Responses name the stylesheet their markup was written for. A tab open
// across an upgrade swaps it in rather than drawing new markup with the old
// one, and keeps playing.
const stylesheet = body.dataset["stylesheet"] ?? "";
const sheet = all<HTMLLinkElement>("link[rel=stylesheet]").find((l) => l.getAttribute("href")?.startsWith(stylesheet));
function freshen(r: Response) {
  const v = r.headers.get("X-Koan-Css");
  if (sheet && stylesheet && v && new URL(sheet.href).searchParams.get("v") !== v) {
    const next = new URL(stylesheet, document.baseURI);
    next.searchParams.set("v", v);
    sheet.href = next.href;
  }
}
window.fetch = async (input, init) => {
  const r = await rawFetch(input, init);
  if (sameOrigin(input)) freshen(r);
  if (r.status !== 401 || !sameOrigin(input)) return r;
  if (await renew()) {
    const again = await rawFetch(input, init);
    freshen(again);
    if (again.status !== 401) return again;
  }
  location.reload();
  return r;
};
document.addEventListener("visibilitychange", () => {
  if (!document.hidden && Date.now() - renewedAt > RENEW_EVERY) renew();
});

// --- Player ----------------------------------------------------------------
const STORE = "koan.queue";
const PLACE = "koan.place";
// Most a queue too large for storage keeps: the playing track and those after.
const KEPT = 2000;
let storedKey: string | null = null;
// The part of the queue stored: all of it, or what fitted from `storedStart`.
let storedStart = 0;
let storedEnd = Infinity;
function storeQueue(queue: UiTrack[], cur: number) {
  try {
    localStorage.setItem(STORE, JSON.stringify({ queue }));
    storedStart = 0;
    storedEnd = Infinity;
    return;
  } catch {}
  // Past the quota: what is still to play, so a restore does not bring back
  // an older queue.
  const start = Math.max(cur, 0);
  try {
    const kept = queue.slice(start, start + KEPT);
    localStorage.setItem(STORE, JSON.stringify({ queue: kept, start }));
    storedStart = start;
    storedEnd = start + kept.length;
  } catch {
    // Not tried again until the queue changes.
    storedStart = 0;
    storedEnd = Infinity;
    console.warn("koan: the queue is too large to keep for the next visit");
    localStorage.removeItem(STORE);
    localStorage.removeItem(PLACE);
  }
}
const save = (from?: number) => {
  const { queue, cur } = player;
  try {
    const key = queue.map((t) => t.id).join();
    // Stored again when the tracks change, or when the playing one has left
    // the part of a queue too large to store whole.
    const outside = cur >= 0 && (cur < storedStart || cur >= storedEnd);
    if (key !== storedKey || outside) { storedKey = key; storeQueue(queue, cur); }
    localStorage.setItem(PLACE, JSON.stringify({ cur: cur - storedStart, pos: from ?? player.position }));
  } catch {}
};
// Every change the player reports saves (a pause and a seek among them), and
// so does leaving: the clock that saves while playing stops in a hidden tab,
// so hiding it and closing it save where the music had got to.
document.addEventListener("visibilitychange", () => { if (document.hidden) save(); });
window.addEventListener("pagehide", () => save());
const player = new Player<UiTrack>({ onChange: render, renew });

function parseCredit(text: string | undefined): Credit {
  if (!text) return [];
  try {
    return Credit.safeParse(JSON.parse(text)).data ?? [];
  } catch {
    return [];
  }
}

// A track as its row carries it.
function trackOf(li: HTMLElement): UiTrack {
  const d = li.dataset;
  return {
    id: Number(d["id"]) || 0,
    src: d["src"] ?? "",
    dur: Number(d["dur"]) || 0,
    title: d["title"],
    artist: d["artist"],
    album: d["album"],
    cover: d["cover"] || null,
    albumHref: d["albumHref"] ?? "",
    properties: d["properties"] ?? "",
    credit: parseCredit(d["credit"]),
  };
}

// A track's artist credit as it reads: each credited artist's name a link to
// them, the text joining the names plain. A track with no parts is its artist.
const creditOf = (t: UiTrack): Credit => (t.credit.length ? t.credit : t.artist ? [{ text: t.artist, href: "" }] : []);
function linkNode(href: string, text: string) {
  const a = document.createElement("a");
  a.href = href;
  a.textContent = text;
  return a;
}
const creditNodes = (t: UiTrack) =>
  creditOf(t).map(({ text, href }) => (href ? linkNode(href, text) : document.createTextNode(text)));

function render({ queue, cur, playing, track }: Player<UiTrack>) {
  body.classList.toggle("playing", playing);
  for (const el of all("[data-ctl=play]")) el.setAttribute("aria-pressed", String(playing));
  for (const el of all("[data-np=title]")) el.textContent = track ? track.title ?? "" : "Nothing playing";
  for (const el of all("[data-np=artist]")) el.replaceChildren(...(track ? creditNodes(track) : []));
  for (const el of all("[data-np=sep]")) el.hidden = !(track && creditOf(track).length && track.album);
  for (const el of all<HTMLAnchorElement>("a[data-np=album]")) {
    el.textContent = track?.album ?? "";
    if (track && track.albumHref) el.href = track.albumHref;
    else el.removeAttribute("href");
  }
  for (const el of all<HTMLImageElement>("img[data-np=cover]")) {
    const src = track && track.cover;
    el.hidden = !src;
    if (src && el.getAttribute("src") !== src) { el.classList.remove("missing"); el.src = src; }
  }
  for (const el of all("[data-np=len]")) el.textContent = fmt(track ? track.dur : 0);
  for (const el of all<HTMLInputElement>("input[data-ctl=seek]")) el.max = String(track ? track.dur : 0);
  for (const el of all<HTMLProgressElement>("progress[data-np=progress]")) el.max = track && track.dur ? track.dur : 1;
  mark();
  askForQueue(queue);
  tick();
  ticking(playing);
  save();
  // A menu open on a queue row would act on the place it had, once the
  // queue itself changes.
  const key = queue.map((t) => t.id).join();
  if (key !== queueKey && menu?.hasAttribute("data-queued") && menu.matches(":popover-open")) menu.hidePopover();
  queueKey = key;
  void cur;
}
let queueKey = "";

// The rows of the track playing marked, in the library and in the queue.
function mark() {
  const { track, cur } = player;
  for (const li of all("li[data-id]", main())) {
    const queued = li.dataset["q"];
    const on = queued !== undefined ? Number(queued) === cur : !!track && Number(li.dataset["id"]) === track.id;
    li.classList.toggle("playing", on);
  }
}

// The queue page's list asks the server for the rows of the queue held here
// whenever the two differ. The list names the queue it was drawn for.
function askForQueue(queue: UiTrack[]) {
  const list = document.getElementById("queue");
  if (!list) return;
  const key = queue.map((t) => t.id).join(",");
  if (list.dataset["drawn"] === key || list.dataset["asked"] === key) return;
  list.dataset["asked"] = key;
  window.dispatchEvent(new CustomEvent("koan-queue", { detail: queue.map((t) => t.id) }));
}

// Pages arrive in place: mark what plays on each, ask for the queue's rows,
// and open the sort and filter row on a wide screen.
new MutationObserver(() => {
  mark();
  askForQueue(player.queue);
  for (const d of all<HTMLDetailsElement>("details.browse", main())) if (wide.matches && !d.dataset["opened"]) { d.dataset["opened"] = "1"; d.open = true; }
}).observe(body, { childList: true, subtree: true });

const dragging = dragged("input[data-ctl=seek]");

let lastSave = 0;
function tick() {
  const { track } = player;
  const p = track ? player.position : 0;
  for (const el of all("[data-np=pos]")) el.textContent = fmt(p);
  for (const el of all<HTMLInputElement>("input[data-ctl=seek]")) {
    if (el !== dragging()) el.value = String(p);
    fill(el);
  }
  for (const el of all<HTMLProgressElement>("progress[data-np=progress]")) el.value = p;
  if (track && Date.now() - lastSave > 5000) { lastSave = Date.now(); save(p); }
}
const ticking = clock(tick);

const shuffle = (a: UiTrack[]) => {
  for (let i = a.length - 1; i > 0; i--) {
    const j = Math.floor(Math.random() * (i + 1));
    const t = a[i];
    const u = a[j];
    if (t && u) { a[i] = u; a[j] = t; }
  }
  return a;
};
const rowsOf = (el: Element) => all("li[data-id]", el.closest("ol") ?? main());

function act(name: string | undefined, el: Element) {
  const albumRows = () => all("ol[data-context=album] li[data-id]", main()).map(trackOf);
  const row = el.closest<HTMLElement>("li");
  switch (name) {
    case "play": return player.play(albumRows(), 0);
    case "shuffle": return player.play(shuffle(albumRows()), 0);
    case "queue": return player.append(albumRows());
    case "add": return row && player.append([trackOf(row)]);
    case "menu": {
      if (!row) return;
      const r = el.getBoundingClientRect();
      return openMenu(row, r.right, r.bottom);
    }
    case "remove": return row && player.remove(Number(row.dataset["q"]));
    case "clear": return player.clear();
  }
}

function pick(li: HTMLElement) {
  const queued = li.dataset["q"];
  if (queued !== undefined) return player.playAt(Number(queued));
  const ol = li.closest("ol");
  if (ol && ol.dataset["context"] === "album") {
    const rows = rowsOf(li);
    return player.play(rows.map(trackOf), rows.indexOf(li));
  }
  player.playNow([trackOf(li)]);
}

// Load a page in place, from an address the server wrote into a row.
const go = (href: string) => { if (href) window.dispatchEvent(new CustomEvent("koan-go", { detail: href })); };

// --- Track menu ------------------------------------------------------------
// What the apps' context menu offers on a track, for any track row: opened by
// right-click, a long press, or the row's ⋯ on a phone. Favourite, share and
// forget press the row's or the page's own buttons, so they behave as those do.
const menu = document.getElementById("track-menu");
function item(label: string, run: () => void) {
  const b = document.createElement("button");
  b.type = "button";
  b.className = "quiet";
  b.setAttribute("role", "menuitem");
  b.textContent = label;
  b.addEventListener("click", () => { menu?.hidePopover(); run(); });
  return b;
}
const group = (items: HTMLElement[]) => (items.length ? [document.createElement("hr"), ...items] : []);
function openMenu(li: HTMLElement, x: number, y: number) {
  if (!menu) return;
  const t = trackOf(li);
  const heart = li.querySelector<HTMLElement>("[data-fav]");
  const share = li.querySelector<HTMLElement>("[data-share]");
  const mine = [];
  if (heart) {
    const on = heart.getAttribute("aria-pressed") === "true";
    mine.push(item(on ? "Remove Favourite" : "Favourite Track", () => heart.click()));
  }
  if (share) mine.push(item("Share Track", () => share.click()));
  // A play in History: forgotten through the page's own Forget, as the
  // apps' Remove from History is, so it is the one way plays leave.
  const play = li.closest("#history") && li.querySelector<HTMLInputElement>("input[type=checkbox][value]");
  const forget = play
    ? [item("Remove from History", () => {
        for (const box of all<HTMLInputElement>("#history input:checked")) box.checked = false;
        play.checked = true;
        play.dispatchEvent(new Event("change", { bubbles: true }));
        document.querySelector<HTMLElement>("[data-forget]")?.click();
      })]
    : [];
  const queued = li.dataset["q"] !== undefined
    ? [item("Remove from Queue", () => player.remove(Number(li.dataset["q"])))]
    : [];
  const places = [];
  if (t.albumHref) places.push(item("Go to Album", () => go(t.albumHref)));
  // Each artist a collaboration credits, by name where there are several.
  const artists = creditOf(t).filter(({ href }) => href);
  for (const { text, href } of artists) {
    places.push(item(artists.length === 1 ? "Go to Artist" : `Go to ${text}`, () => go(href)));
  }
  const about = t.properties ? [item("Properties…", () => go(t.properties))] : [];
  const head = document.createElement("p");
  head.textContent = [t.title, t.artist].filter(Boolean).join(" · ");
  menu.replaceChildren(
    head,
    item("Play", () => pick(li)),
    item("Play Next", () => player.playNext([t])),
    item("Add to Queue", () => player.append([t])),
    ...group(mine),
    ...group(places),
    ...group(forget),
    ...group(queued),
    ...group(about),
  );
  // A queue row's place moves when the queue does; see render.
  menu.toggleAttribute("data-queued", li.dataset["q"] !== undefined);
  if (!menu.matches(":popover-open")) menu.showPopover();
  if (wide.matches) {
    const r = menu.getBoundingClientRect();
    menu.style.left = `${Math.max(8, Math.min(x, innerWidth - r.width - 8))}px`;
    menu.style.top = `${Math.max(8, Math.min(y, innerHeight - r.height - 8))}px`;
  } else {
    menu.style.left = menu.style.top = "";
  }
  menu.querySelector("button")?.focus();
}
// A manual popover: the press that opens it ends on the row (a touch stays
// captured by what it went down on), which a light-dismissing popover would
// take for a click outside. It closes on the next press outside it, Escape,
// or leaving the page; the press that closes it does nothing else, as with
// the apps' menus.
let dismissed = false;
const closeMenu = () => {
  if (!menu || !menu.matches(":popover-open")) return false;
  menu.hidePopover();
  return true;
};
document.addEventListener("pointerdown", (e) => {
  if (menu && !(e.target instanceof Node && menu.contains(e.target)) && closeMenu()) dismissed = true;
}, true);
document.addEventListener("keydown", (e) => { if (e.key === "Escape") closeMenu(); });
const menuRow = (e: Event) => e.target instanceof Element && e.target.closest<HTMLElement>("li[data-id]");
document.addEventListener("contextmenu", (e) => {
  const li = menuRow(e);
  if (!li || !(e.target instanceof Element) || e.target.closest("a:not(.t a), input")) return;
  e.preventDefault();
  // The keyboard's menu key has no pointer: open it at the row.
  const r = li.getBoundingClientRect();
  openMenu(li, e.clientX || r.left + 24, e.clientY || r.bottom);
});
// iOS fires no contextmenu on a long press, so a touch held still opens it,
// and the click that follows the lift is not a pick.
let press = 0, pressed = false, pressAt: [number, number] | null = null;
document.addEventListener("pointerdown", (e) => {
  const li = e.pointerType === "touch" && menuRow(e);
  if (!li || !(e.target instanceof Element) || e.target.closest("a:not(.t a), button, input, label")) return;
  pressAt = [e.clientX, e.clientY];
  pressed = false;
  press = setTimeout(() => { pressed = true; openMenu(li, e.clientX, e.clientY); }, 500);
});
document.addEventListener("pointermove", (e) => {
  if (pressAt && Math.hypot(e.clientX - pressAt[0], e.clientY - pressAt[1]) > 10) clearTimeout(press);
});
for (const type of ["pointerup", "pointercancel", "scroll"]) {
  document.addEventListener(type, () => {
    clearTimeout(press);
    pressAt = null;
    // Not every browser sends that click, so the guard lapses on its own.
    if (pressed || dismissed) setTimeout(() => { pressed = dismissed = false; }, 350);
  }, { passive: true, capture: true });
}
document.addEventListener("click", (e) => {
  if (pressed || dismissed) { pressed = dismissed = false; e.preventDefault(); e.stopPropagation(); }
}, true);

// --- Invites ---------------------------------------------------------------
// The email goes out as rich text where the clipboard takes it, so pasting
// into a mail client keeps the button. Datastar can write only plain text.
async function copyEmail(button: Element) {
  const email = document.getElementById("invite-email");
  const html = email instanceof HTMLTemplateElement ? email.innerHTML : "";
  const invite = document.getElementById("invite-text");
  const text = invite instanceof HTMLTextAreaElement ? invite.value : "";
  try {
    await navigator.clipboard.write([new ClipboardItem({
      "text/html": new Blob([html], { type: "text/html" }),
      "text/plain": new Blob([text], { type: "text/plain" }),
    })]);
  } catch {
    await navigator.clipboard.writeText(text);
  }
  button.textContent = "Copied";
}
document.addEventListener("click", (e) => {
  if (!(e.target instanceof Element)) return;
  const email = e.target.closest("[data-copy-email]");
  if (email) { copyEmail(email); return; }
  const ctl = e.target.closest<HTMLElement>("[data-ctl]");
  if (ctl) {
    const which = ctl.dataset["ctl"];
    if (which === "play") player.toggle();
    else if (which === "prev") player.prev();
    else if (which === "next") player.next();
    return;
  }
  const a = e.target.closest<HTMLElement>("[data-act]");
  if (a) { e.preventDefault(); act(a.dataset["act"], a); return; }
  const li = e.target.closest<HTMLElement>("li[data-id]");
  if (li && !e.target.closest("a, button, input, label")) pick(li);
});
document.addEventListener("keydown", (e) => {
  if (e.key !== "Enter" && e.key !== " ") return;
  const li = e.target instanceof Element && e.target.closest<HTMLElement>("li[data-id]");
  if (li && e.target === li) { e.preventDefault(); pick(li); }
});
document.addEventListener("input", (e) => {
  if (e.target instanceof HTMLInputElement && e.target.matches("input[data-ctl=seek]")) {
    for (const el of all("[data-np=pos]")) el.textContent = fmt(Number(e.target.value));
    fill(e.target);
  }
});
document.addEventListener("change", (e) => {
  if (e.target instanceof HTMLInputElement && e.target.matches("input[data-ctl=seek]")) player.seek(Number(e.target.value));
});
// A cover that is not there leaves an empty tile rather than a broken image.
document.addEventListener("error", (e) => {
  if (!(e.target instanceof HTMLImageElement)) return;
  e.target.classList.add("missing");
  // With no source the image draws as its own empty box, not a broken icon.
  e.target.removeAttribute("src");
}, true);

// The sort and filter row stands open on a wide screen; on a phone it is one
// disclosure that opens as a sheet.
const wide = matchMedia("(min-width: 721px)");

// Pick up where the last visit left off, paused.
function restore() {
  try {
    const saved = Saved.safeParse(JSON.parse(localStorage.getItem(STORE) || "null")).data;
    if (!saved) return;
    const place = Place.safeParse(JSON.parse(localStorage.getItem(PLACE) || "null")).data;
    const queue: UiTrack[] = saved.queue.flatMap((raw) => {
      const t = Stored.safeParse(raw).data;
      if (!t) return [];
      return [{
        id: t.id, src: t.src, dur: t.dur,
        title: t.title ?? undefined, artist: t.artist ?? undefined, album: t.album ?? undefined, cover: t.cover ?? null,
        albumHref: t.albumHref ?? "", properties: t.properties ?? "", credit: t.credit ?? [],
      }];
    });
    storedKey = saved.start ? null : queue.map((t) => t.id).join();
    const cur = place ? place.cur : saved.cur ?? -1;
    player.set(queue, Math.max(cur, -1), (place ? place.pos : saved.pos) || 0);
  } catch {}
}
restore();
for (const d of all<HTMLDetailsElement>("details.browse")) if (wide.matches) { d.dataset["opened"] = "1"; d.open = true; }
render(player);
