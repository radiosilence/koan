// The web UI's own script: navigation that swaps only the page content so the
// player keeps playing, the transport and queue, and keeping the session alive.
(() => {
  const main = document.getElementById("content");
  document.documentElement.classList.add("js");
  // The server groups history by day, and only the browser knows whose day.
  document.cookie = `koan_tz=${-new Date().getTimezoneOffset()}; path=/; max-age=31536000; samesite=lax`;
  const all = (sel, root = document) => [...root.querySelectorAll(sel)];
  const fmt = (s) => {
    s = Math.max(0, Math.floor(s || 0));
    return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
  };

  // --- Session ---------------------------------------------------------------
  // The access cookie lasts minutes. Renewing spends the refresh cookie, which
  // only /auth routes receive, for fresh ones; no token is ever visible here.
  const rawFetch = window.fetch.bind(window);
  let renewing = null;
  let renewedAt = Date.now();
  function renew() {
    renewing ??= rawFetch("/auth/renew", { method: "POST", credentials: "same-origin" })
      .then((r) => { if (r.ok) renewedAt = Date.now(); return r.ok; })
      .catch(() => false)
      .finally(() => { renewing = null; });
    return renewing;
  }
  const sameOrigin = (input) =>
    new URL(typeof input === "string" ? input : input.url, location.href).origin === location.origin;
  // Every request this page makes, Datastar's included, renews once on a 401
  // and retries; a session that cannot be renewed reloads into the sign-in form.
  // Responses name the stylesheet their markup was written for. A tab open
  // across an upgrade swaps it in rather than drawing new markup with the old
  // one, and keeps playing.
  const sheet = document.querySelector('link[rel=stylesheet][href^="/ui/assets/ui.css"]');
  function freshen(r) {
    const v = r.headers.get("X-Koan-Css");
    if (sheet && v && new URL(sheet.href).searchParams.get("v") !== v) {
      sheet.href = `/ui/assets/ui.css?v=${v}`;
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
  const RENEW_EVERY = 10 * 60 * 1000;
  setInterval(() => { if (Date.now() - renewedAt > RENEW_EVERY) renew(); }, 60 * 1000);
  document.addEventListener("visibilitychange", () => {
    if (!document.hidden && Date.now() - renewedAt > RENEW_EVERY) renew();
  });

  // --- Player ----------------------------------------------------------------
  const STORE = "koan.queue";
  const save = (from) => {
    const { queue, cur } = player.state();
    try { localStorage.setItem(STORE, JSON.stringify({ queue, cur, pos: from ?? player.position() })); } catch {}
  };
  const player = window.KoanPlayer({ onChange: render, renew });

  function trackOf(li) {
    const d = li.dataset;
    const albumId = Number(d.albumId) || 0;
    return {
      id: Number(d.id), src: `/ui/stream/${d.id}`, dur: Number(d.dur) || 0,
      title: d.title, artist: d.artist, album: d.album, albumId,
      cover: d.cover || null,
    };
  }

  function render({ queue, cur, playing, track }) {
    document.body.classList.toggle("playing", playing);
    for (const el of all("[data-np=title]")) el.textContent = track ? track.title : "Nothing playing";
    for (const el of all("[data-np=artist]")) el.textContent = track ? track.artist : "";
    for (const el of all("[data-np=album]")) {
      el.textContent = track ? track.album : "";
      el.href = track && track.albumId ? `/album/${track.albumId}` : "/albums";
    }
    for (const el of all("img[data-np=cover]")) {
      const src = track && track.cover;
      el.hidden = !src;
      if (src && el.getAttribute("src") !== src) { el.classList.remove("missing"); el.src = src; }
    }
    for (const el of all("[data-np=len]")) el.textContent = fmt(track ? track.dur : 0);
    for (const el of all("input[data-ctl=seek]")) el.max = track ? track.dur : 0;
    for (const el of all("progress[data-np=progress]")) el.max = track && track.dur ? track.dur : 1;
    for (const li of all("li[data-id]", main)) li.classList.toggle("playing", !!track && Number(li.dataset.id) === track.id);
    renderQueue(queue, cur);
    tick();
    save();
  }

  function renderQueue(queue, cur) {
    const list = document.getElementById("queue-list");
    if (!list) return;
    list.replaceChildren();
    if (!queue.length) {
      const p = document.createElement("li");
      p.className = "disc";
      p.textContent = "The queue is empty. Play an album to fill it.";
      list.append(p);
      return;
    }
    queue.forEach((t, i) => {
      const li = document.createElement("li");
      li.tabIndex = 0;
      li.dataset.q = i;
      li.classList.toggle("playing", i === cur);
      const n = document.createElement("span");
      n.className = "n";
      n.textContent = i + 1;
      const title = document.createElement("span");
      title.className = "t";
      title.textContent = t.title;
      const small = document.createElement("small");
      small.textContent = [t.artist, t.album].filter(Boolean).join(" · ");
      title.append(small);
      const d = document.createElement("span");
      d.className = "d";
      d.textContent = fmt(t.dur);
      const rm = document.createElement("button");
      rm.className = "quiet";
      rm.dataset.act = "remove";
      rm.setAttribute("aria-label", "Remove from queue");
      rm.title = "Remove from queue";
      rm.textContent = "×";
      li.append(n, title, d, rm);
      list.append(li);
    });
  }

  // The range being dragged, which the clock must not move under the finger.
  // Focus is no guide: a clicked range keeps it long after the drag ends.
  let dragging = null;
  const isSeek = (t) => t && t.matches && t.matches("input[data-ctl=seek]");
  for (const type of ["pointerdown", "touchstart"]) {
    document.addEventListener(type, (e) => { if (isSeek(e.target)) dragging = e.target; }, { passive: true });
  }
  for (const type of ["pointerup", "pointercancel", "touchend", "touchcancel", "change", "focusout"]) {
    document.addEventListener(type, () => { dragging = null; }, { passive: true });
  }

  let lastSave = 0;
  function tick() {
    const { track } = player.state();
    const p = track ? player.position() : 0;
    for (const el of all("[data-np=pos]")) el.textContent = fmt(p);
    for (const el of all("input[data-ctl=seek]")) if (el !== dragging) el.value = p;
    for (const el of all("progress[data-np=progress]")) el.value = p;
    if (track && Date.now() - lastSave > 5000) { lastSave = Date.now(); save(p); }
  }
  setInterval(tick, 250);

  const shuffle = (a) => {
    for (let i = a.length - 1; i > 0; i--) {
      const j = Math.floor(Math.random() * (i + 1));
      [a[i], a[j]] = [a[j], a[i]];
    }
    return a;
  };
  const rowsOf = (el) => all("li[data-id]", el.closest("ol") || el.closest(".page") || main);

  function act(name, el) {
    const page = el.closest(".page");
    const albumRows = () => (page ? all("ol[data-context=album] li[data-id]", page) : []).map(trackOf);
    const row = el.closest("li");
    switch (name) {
      case "play": return player.play(albumRows(), 0);
      case "shuffle": return player.play(shuffle(albumRows()), 0);
      case "queue": return player.append(albumRows());
      case "add": return player.append([trackOf(row)]);
      case "menu": {
        const r = el.getBoundingClientRect();
        return openMenu(row, r.right, r.bottom);
      }
      case "remove": return player.remove(Number(row.dataset.q));
      case "clear": return player.clear();
    }
  }

  function pick(li) {
    if (li.dataset.q !== undefined) return player.playAt(Number(li.dataset.q));
    const ol = li.closest("ol");
    if (ol && ol.dataset.context === "album") {
      const rows = rowsOf(li);
      return player.play(rows.map(trackOf), rows.indexOf(li));
    }
    player.playNow([trackOf(li)]);
  }

  // --- Track menu ------------------------------------------------------------
  // What the apps' context menu offers on a track, for any track row: opened by
  // right-click, a long press, or the row's ⋯ on a phone. Favourite and share
  // press the row's own buttons, so they behave as those do.
  const menu = document.getElementById("track-menu");
  function item(label, run) {
    const b = document.createElement("button");
    b.className = "quiet";
    b.setAttribute("role", "menuitem");
    b.textContent = label;
    b.addEventListener("click", () => { menu.hidePopover(); run(); });
    return b;
  }
  const group = (items) => (items.length ? [document.createElement("hr"), ...items] : []);
  function openMenu(li, x, y) {
    if (!menu) return;
    const t = trackOf(li);
    const d = li.dataset;
    const heart = li.querySelector("[data-fav]");
    const share = li.querySelector("[data-act-share]");
    const mine = [];
    if (heart) {
      const on = heart.getAttribute("aria-pressed") === "true";
      mine.push(item(on ? "Remove Favourite" : "Favourite Track", () => heart.click()));
    }
    if (share) mine.push(item("Share Track", () => share.click()));
    // A play in History: forgotten through the page's own Forget, as the
    // apps' Remove from History is, so it is the one way plays leave.
    const play = li.closest("#history") && li.querySelector("input[type=checkbox][value]");
    const forget = play
      ? [item("Remove from History", () => {
          for (const box of all("#history input:checked")) box.checked = false;
          play.checked = true;
          play.dispatchEvent(new Event("change", { bubbles: true }));
          document.querySelector("[data-act-forget]")?.click();
        })]
      : [];
    const go = [];
    if (Number(d.albumId)) go.push(item("Go to Album", () => navigate(`/album/${d.albumId}`, true)));
    if (Number(d.artistId)) go.push(item("Go to Artist", () => navigate(`/artist/${d.artistId}`, true)));
    const head = document.createElement("p");
    head.textContent = [t.title, t.artist].filter(Boolean).join(" · ");
    menu.replaceChildren(
      head,
      item("Play", () => pick(li)),
      item("Play Next", () => player.playNext([t])),
      item("Add to Queue", () => player.append([t])),
      ...group(mine),
      ...group(go),
      ...group(forget),
    );
    if (!menu.matches(":popover-open")) menu.showPopover();
    if (wide.matches) {
      const r = menu.getBoundingClientRect();
      menu.style.left = `${Math.max(8, Math.min(x, innerWidth - r.width - 8))}px`;
      menu.style.top = `${Math.max(8, Math.min(y, innerHeight - r.height - 8))}px`;
    } else {
      menu.style.left = menu.style.top = "";
    }
    menu.querySelector("button").focus();
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
    if (menu && !menu.contains(e.target) && closeMenu()) dismissed = true;
  }, true);
  document.addEventListener("keydown", (e) => { if (e.key === "Escape") closeMenu(); });
  const menuRow = (e) => e.target.closest && e.target.closest("li[data-id]");
  document.addEventListener("contextmenu", (e) => {
    const li = menuRow(e);
    if (!li || e.target.closest("a, input")) return;
    e.preventDefault();
    // The keyboard's menu key has no pointer: open it at the row.
    const r = li.getBoundingClientRect();
    openMenu(li, e.clientX || r.left + 24, e.clientY || r.bottom);
  });
  // iOS fires no contextmenu on a long press, so a touch held still opens it,
  // and the click that follows the lift is not a pick.
  let press = 0, pressed = false, pressAt = null;
  document.addEventListener("pointerdown", (e) => {
    const li = e.pointerType === "touch" && menuRow(e);
    if (!li || e.target.closest("a, button, input, label")) return;
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
  // into a mail client keeps the button; the share sheet gets the plain text.
  const inviteEmail = () => ({
    html: document.getElementById("invite-html")?.innerHTML ?? "",
    text: document.getElementById("invite-text")?.value ?? "",
    subject: document.getElementById("invite-text")?.dataset.subject ?? "",
  });
  async function copyEmail(button) {
    const { html, text } = inviteEmail();
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
  function inviteClick(e) {
    const copy = e.target.closest("[data-copy]");
    if (copy) {
      const el = document.getElementById(copy.dataset.copy);
      navigator.clipboard.writeText(el.value ?? el.textContent).then(() => { copy.textContent = "Copied"; });
      return true;
    }
    const email = e.target.closest("[data-copy-email]");
    if (email) { copyEmail(email); return true; }
    const share = e.target.closest("[data-share-email]");
    if (share) {
      const { text, subject } = inviteEmail();
      navigator.share({ title: subject, text }).catch(() => {});
      return true;
    }
    return false;
  }

  document.addEventListener("click", (e) => {
    if (inviteClick(e)) return;
    const ctl = e.target.closest("[data-ctl]");
    if (ctl) {
      if (ctl.dataset.ctl === "play") player.toggle();
      else if (ctl.dataset.ctl === "prev") player.prev();
      else if (ctl.dataset.ctl === "next") player.next();
      return;
    }
    const a = e.target.closest("[data-act]");
    if (a) { e.preventDefault(); act(a.dataset.act, a); return; }
    const li = e.target.closest("li[data-id], li[data-q]");
    if (li && !e.target.closest("a, button, input, label")) pick(li);
  });
  document.addEventListener("keydown", (e) => {
    if (e.key !== "Enter" && e.key !== " ") return;
    const li = e.target.closest && e.target.closest("li[data-id], li[data-q]");
    if (li && e.target === li) { e.preventDefault(); pick(li); }
  });
  document.addEventListener("input", (e) => {
    if (e.target.matches("input[data-ctl=seek]")) {
      for (const el of all("[data-np=pos]")) el.textContent = fmt(Number(e.target.value));
    }
  });
  document.addEventListener("change", (e) => {
    if (e.target.matches("input[data-ctl=seek]")) player.seek(Number(e.target.value));
  });
  document.addEventListener("focusin", (e) => { if (e.target.id === "share-url") e.target.select(); });
  // A cover that is not there leaves an empty tile rather than a broken image.
  document.addEventListener("error", (e) => {
    if (e.target.tagName !== "IMG") return;
    e.target.classList.add("missing");
    // With no source the image draws as its own empty box, not a broken icon.
    e.target.removeAttribute("src");
  }, true);

  // --- Navigation ------------------------------------------------------------
  const INTERNAL = /^\/(albums|tracks|album\/\d+|artists|artist\/\d+|search|queue|account|library|favourites|history|recent|scrobbling)?$/;
  let navigating = 0;

  async function navigate(url, push) {
    const n = ++navigating;
    let r;
    try {
      r = await fetch(url, { headers: { "X-Koan-Partial": "1" }, credentials: "same-origin" });
    } catch {
      location.href = url;
      return;
    }
    if (n !== navigating) return;
    if (!r.ok) { location.href = url; return; }
    const html = await r.text();
    if (n !== navigating) return;
    closeMenu();
    main.innerHTML = html;
    if (push) history.pushState(null, "", url);
    settle();
    scrollTo(0, 0);
  }

  function settle() {
    const page = main.firstElementChild;
    const title = page && page.dataset.title;
    document.title = title ? `${title} · koan` : "koan";
    const section = { "": "albums", album: "albums", artist: "artists", playlist: "playlists" }[location.pathname.split("/")[1]]
      ?? location.pathname.split("/")[1];
    for (const a of all("a[data-nav]")) {
      // A phone's Library tab stands for several of the sidebar's links.
      if (a.dataset.nav.split(" ").includes(section)) a.setAttribute("aria-current", "page");
      else a.removeAttribute("aria-current");
    }
    const focus = main.querySelector("[autofocus]");
    if (focus) focus.focus();
    // The sort and filter row stands open on a wide screen; on a phone it is
    // one button that opens as a sheet.
    for (const d of all("details.browse", main)) d.open = wide.matches;
    render(player.state());
  }

  // --- Sort and filter ---------------------------------------------------------
  // The toolbar is a GET form; its state is the URL. On a wide screen a change
  // applies at once, on a phone the sheet's Apply does.
  const wide = matchMedia("(min-width: 721px)");
  function formUrl(form) {
    const params = new URLSearchParams();
    for (const [k, v] of new FormData(form)) if (String(v).trim()) params.append(k, v);
    const q = params.toString();
    return form.getAttribute("action") + (q ? `?${q}` : "");
  }
  document.addEventListener("change", (e) => {
    const form = e.target.closest && e.target.closest("form.toolbar");
    if (!form || !wide.matches || e.target.matches(TYPED)) return;
    navigate(formUrl(form), true);
  });
  document.addEventListener("submit", (e) => {
    if (!e.target.matches("form.toolbar")) return;
    e.preventDefault();
    clearTimeout(typing);
    navigate(formUrl(e.target), true);
  });
  // Typing filters as it goes, once the keys pause. Only the listing below the
  // toolbar is replaced, so the field keeps its focus and caret; the URL is
  // replaced rather than pushed, so back does not step through every keystroke.
  // A year applies only when it is whole, or cleared.
  const TYPED = "form.toolbar input[name=q], form.toolbar input[name=from], form.toolbar input[name=to]";
  let typing;
  document.addEventListener("input", (e) => {
    if (!wide.matches || !e.target.matches(TYPED)) return;
    const v = e.target.value.trim();
    if (e.target.name !== "q" && v && !/^\d{4}$/.test(v)) return;
    clearTimeout(typing);
    const form = e.target.form;
    typing = setTimeout(() => refilter(formUrl(form)), 250);
  });
  async function refilter(url) {
    const n = ++navigating;
    let r;
    try {
      r = await fetch(url, { headers: { "X-Koan-Partial": "1" }, credentials: "same-origin" });
    } catch {
      return;
    }
    if (n !== navigating || !r.ok) return;
    const html = await r.text();
    if (n !== navigating) return;
    const next = document.createElement("template");
    next.innerHTML = html;
    const bar = main.querySelector("details.browse");
    const fresh = next.content.querySelector("details.browse");
    if (!bar || !fresh) return;
    while (bar.nextSibling) bar.nextSibling.remove();
    while (fresh.nextSibling) bar.parentNode.append(fresh.nextSibling);
    history.replaceState(null, "", url);
    render(player.state());
  }
  // The search view's query lives in the URL too, so reload and back find it.
  document.addEventListener("input", (e) => {
    if (!e.target.matches(".search input[name=q]")) return;
    const q = e.target.value.trim();
    history.replaceState(null, "", q ? `/search?q=${encodeURIComponent(q)}` : "/search");
  });

  document.addEventListener("click", (e) => {
    if (e.defaultPrevented || e.button !== 0 || e.metaKey || e.ctrlKey || e.shiftKey || e.altKey) return;
    const a = e.target.closest("a[href]");
    if (!a || a.target || a.hasAttribute("download")) return;
    const url = new URL(a.href, location.href);
    if (url.origin !== location.origin || !INTERNAL.test(url.pathname)) return;
    e.preventDefault();
    if (url.pathname + url.search !== location.pathname + location.search) navigate(url.pathname + url.search, true);
  });
  addEventListener("popstate", () => navigate(location.pathname + location.search, false));

  // Pick up where the last visit left off, paused.
  try {
    const saved = JSON.parse(localStorage.getItem(STORE) || "null");
    if (saved && Array.isArray(saved.queue)) player.set(saved.queue, saved.cur, saved.pos || 0);
  } catch {}
  settle();
})();
