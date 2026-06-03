// homelab-hub client behaviour: theme toggle, live refresh, window selector.
(function () {
  "use strict";

  // Theme toggle.
  const TKEY = "homelab-hub-theme";
  const order = ["auto", "light", "dark"];
  function applyTheme(t) {
    const root = document.documentElement;
    if (t === "auto") root.removeAttribute("data-theme");
    else root.setAttribute("data-theme", t);
  }
  const savedTheme = localStorage.getItem(TKEY);
  if (savedTheme && order.includes(savedTheme)) applyTheme(savedTheme);

  const toggle = document.getElementById("theme-toggle");
  if (toggle) {
    toggle.addEventListener("click", function () {
      const current = localStorage.getItem(TKEY) || "auto";
      const next = order[(order.indexOf(current) + 1) % order.length];
      localStorage.setItem(TKEY, next);
      applyTheme(next);
      toggle.title = "Theme: " + next;
    });
  }

  // "Check now" button, on both the dashboard and detail page.
  document.addEventListener("click", function (ev) {
    const btn = ev.target.closest(".check-now");
    if (!btn) return;
    ev.preventDefault();
    const id = btn.dataset.service;
    btn.classList.add("spinning");
    fetch("/api/services/" + encodeURIComponent(id) + "/check", { method: "POST" })
      .catch(function () {})
      .finally(function () {
        setTimeout(function () {
          if (window.__hubRefresh) window.__hubRefresh();
          else location.reload();
        }, 1500);
      });
  });

  // Live dashboard.
  const dash = document.getElementById("dashboard");
  if (!dash) return;

  const WKEY = "homelab-hub-window";
  let currentWindow = localStorage.getItem(WKEY) || dash.dataset.window || "24h";
  const refreshSecs = Math.max(parseInt(dash.dataset.refresh || "15", 10) || 15, 3);

  // Filtering (transient, in-memory).
  const filterInput = document.getElementById("filter");
  let currentFilter = "";
  function applyFilter() {
    const q = currentFilter.trim().toLowerCase();
    dash.querySelectorAll(".card").forEach(function (card) {
      const hay = card.getAttribute("data-filter") || "";
      card.classList.toggle("hidden", q !== "" && !hay.includes(q));
    });
    // Hide groups with no visible cards.
    dash.querySelectorAll(".group").forEach(function (sec) {
      sec.classList.toggle("hidden", !sec.querySelector(".card:not(.hidden)"));
    });
  }
  if (filterInput) {
    filterInput.addEventListener("input", function () {
      currentFilter = filterInput.value;
      applyFilter();
    });
  }

  // Collapsible groups (persisted).
  const CKEY = "homelab-hub-collapsed";
  function loadCollapsed() {
    try {
      return new Set(JSON.parse(localStorage.getItem(CKEY) || "[]"));
    } catch (e) {
      return new Set();
    }
  }
  let collapsed = loadCollapsed();
  function applyCollapse() {
    dash.querySelectorAll(".group").forEach(function (sec) {
      sec.classList.toggle("collapsed", collapsed.has(sec.getAttribute("data-group") || ""));
    });
  }
  dash.addEventListener("click", function (ev) {
    const title = ev.target.closest(".group-title");
    if (!title) return;
    const sec = title.closest(".group");
    const name = sec.getAttribute("data-group") || "";
    if (collapsed.has(name)) collapsed.delete(name);
    else collapsed.add(name);
    localStorage.setItem(CKEY, JSON.stringify([...collapsed]));
    sec.classList.toggle("collapsed");
  });

  let refreshing = false;
  let queued = false;

  async function refresh() {
    if (refreshing) {
      queued = true;
      return;
    }
    refreshing = true;
    try {
      const res = await fetch(
        "/partials/dashboard?window=" + encodeURIComponent(currentWindow)
      );
      if (res.ok) {
        dash.innerHTML = await res.text();
        applyCollapse();
        applyFilter();
        const updated = document.getElementById("updated");
        if (updated) updated.textContent = "updated " + new Date().toLocaleTimeString();
      }
    } catch (e) {
      /* keep last rendered view on transient errors */
    }
    refreshing = false;
    if (queued) {
      queued = false;
      refresh();
    }
  }
  window.__hubRefresh = refresh;

  // Window selector via event delegation so it survives innerHTML swaps.
  dash.addEventListener("click", function (ev) {
    const btn = ev.target.closest(".window-btn");
    if (!btn) return;
    currentWindow = btn.dataset.window;
    localStorage.setItem(WKEY, currentWindow);
    refresh();
  });

  // Periodic polling (fallback / catch-all).
  setInterval(refresh, refreshSecs * 1000);

  // Server-Sent Events: refresh promptly on a reported change (debounced).
  if (window.EventSource) {
    try {
      const es = new EventSource("/events");
      let timer = null;
      es.onmessage = function () {
        clearTimeout(timer);
        timer = setTimeout(refresh, 400);
      };
    } catch (e) {
      /* polling still covers us */
    }
  }

  // Apply persisted collapse state to the server-rendered groups on load.
  applyCollapse();

  // If a different window was remembered than the server rendered, sync now.
  if (currentWindow !== (dash.dataset.window || "24h")) refresh();
})();
