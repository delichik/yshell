/* YShell prototype embed helpers — query params + theme sync.
   Usage: include after DOM; call ProtoEmbed.boot() or rely on auto-boot. */
(function (global) {
  var THEME_KEY = "yshell-proto-theme";
  var MSG = "yshell-proto-theme";

  function qs() {
    return new URLSearchParams(global.location.search);
  }

  function applyTheme(theme) {
    if (theme === "dark") {
      document.documentElement.setAttribute("data-theme", "dark");
    } else {
      document.documentElement.removeAttribute("data-theme");
    }
  }

  function readStoredTheme() {
    try {
      return localStorage.getItem(THEME_KEY) === "dark" ? "dark" : "light";
    } catch (_) {
      return "light";
    }
  }

  function storeTheme(theme) {
    try {
      localStorage.setItem(THEME_KEY, theme === "dark" ? "dark" : "light");
    } catch (_) {}
  }

  /** Apply data-q (textContent) and data-q-href (href) from URLSearchParams. */
  function applyParams(searchParams) {
    var q = searchParams || qs();
    document.querySelectorAll("[data-q]").forEach(function (el) {
      var key = el.getAttribute("data-q");
      if (!key) return;
      var v = q.get(key);
      if (v != null) el.textContent = v;
    });
    document.querySelectorAll("[data-q-href]").forEach(function (el) {
      var key = el.getAttribute("data-q-href");
      if (!key) return;
      var v = q.get(key);
      if (v != null) el.setAttribute("href", v);
    });
    document.querySelectorAll("[data-q-title]").forEach(function (el) {
      var key = el.getAttribute("data-q-title");
      if (!key) return;
      var v = q.get(key);
      if (v != null) el.setAttribute("title", v);
    });

    var tier = q.get("tier");
    if (tier) {
      document.querySelectorAll("[data-tier-target]").forEach(function (el) {
        el.classList.remove("win-compact", "win-standard", "win-wide", "win-main");
        el.classList.add("win-" + tier);
      });
    }

    if (q.get("embed") === "1") {
      document.documentElement.classList.add("proto-embed-root");
      document.body.classList.add("proto-embed-root");
    }

    var theme = q.get("theme");
    if (theme === "dark" || theme === "light") {
      applyTheme(theme);
    }
  }

  function enableProtoThemeToggle(checkboxId) {
    var el = document.getElementById(checkboxId || "theme-toggle");
    var q = qs();
    var initial = q.get("theme") || readStoredTheme();
    applyTheme(initial);
    if (!el) return;
    el.checked = initial === "dark";
    el.addEventListener("change", function () {
      var theme = el.checked ? "dark" : "light";
      applyTheme(theme);
      storeTheme(theme);
      syncChildIframes(theme);
    });
  }

  function syncChildIframes(theme) {
    document.querySelectorAll("iframe.proto-embed, iframe[data-proto-embed]").forEach(function (frame) {
      try {
        if (frame.contentWindow) {
          frame.contentWindow.postMessage({ type: MSG, theme: theme }, "*");
        }
      } catch (_) {}
      var src = frame.getAttribute("src");
      if (!src) return;
      try {
        var u = new URL(src, global.location.href);
        u.searchParams.set("theme", theme);
        var next = u.pathname + u.search + u.hash;
        // Keep relative if original was relative
        if (!/^https?:/i.test(src) && src.indexOf("//") !== 0) {
          var base = src.split("?")[0].split("#")[0];
          next = base + "?" + u.searchParams.toString();
        }
        if (frame.getAttribute("src") !== next) frame.setAttribute("src", next);
      } catch (_) {}
    });
  }

  function listenParentTheme() {
    global.addEventListener("message", function (ev) {
      var d = ev.data;
      if (!d || d.type !== MSG) return;
      if (d.theme === "dark" || d.theme === "light") applyTheme(d.theme);
    });
  }

  function boot(opts) {
    opts = opts || {};
    applyParams();
    listenParentTheme();
    if (opts.toggle !== false) enableProtoThemeToggle(opts.toggleId);
  }

  global.ProtoEmbed = {
    applyParams: applyParams,
    applyTheme: applyTheme,
    enableProtoThemeToggle: enableProtoThemeToggle,
    syncChildIframes: syncChildIframes,
    boot: boot,
  };

  // Back-compat with pages that call enableProtoThemeToggle()
  global.enableProtoThemeToggle = enableProtoThemeToggle;

  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", function () {
      if (document.documentElement.hasAttribute("data-proto-autoboot") ||
          document.body.hasAttribute("data-proto-autoboot")) {
        boot();
      }
    });
  }
})(window);
