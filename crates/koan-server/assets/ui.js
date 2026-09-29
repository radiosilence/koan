// The web UI's own script: navigation that swaps only the page content so the
// player keeps playing, the transport and queue, and keeping the session alive.
(() => {
  const main = document.getElementById("content");
  document.documentElement.classList.add("js");
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
  window.fetch = async (input, init) => {
    const r = await rawFetch(input, init);
    if (r.status !== 401 || !sameOrigin(input)) return r;
    if (await renew()) {
      const again = await rawFetch(input, init);
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
    if (li && !e.target.closest("a, button")) pick(li);
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
  const INTERNAL = /^\/(albums|album\/\d+|artists|artist\/\d+|search|queue|keys)?$/;
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
    main.innerHTML = html;
    if (push) history.pushState(null, "", url);
    settle();
    scrollTo(0, 0);
  }

  function settle() {
    const page = main.firstElementChild;
    const title = page && page.dataset.title;
    document.title = title ? `${title} · koan` : "koan";
    const section = { "": "albums", album: "albums", artist: "artists" }[location.pathname.split("/")[1]]
      ?? location.pathname.split("/")[1];
    for (const a of all("a[data-nav]")) {
      if (a.dataset.nav === section) a.setAttribute("aria-current", "page");
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
    if (form && wide.matches) navigate(formUrl(form), true);
  });
  document.addEventListener("submit", (e) => {
    if (!e.target.matches("form.toolbar")) return;
    e.preventDefault();
    navigate(formUrl(e.target), true);
  });
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
