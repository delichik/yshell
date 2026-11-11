/* Shared helpers for YShell design drafts. */
(function (global) {
  function enableProtoThemeToggle(checkboxId) {
    var el = document.getElementById(checkboxId || "theme-toggle");
    if (!el) return;
    var key = "yshell-proto-theme";
    try {
      if (localStorage.getItem(key) === "dark") {
        document.documentElement.setAttribute("data-theme", "dark");
        el.checked = true;
      }
    } catch (_) {}
    el.addEventListener("change", function () {
      if (el.checked) {
        document.documentElement.setAttribute("data-theme", "dark");
        try { localStorage.setItem(key, "dark"); } catch (_) {}
      } else {
        document.documentElement.removeAttribute("data-theme");
        try { localStorage.setItem(key, "light"); } catch (_) {}
      }
    });
  }
  global.enableProtoThemeToggle = enableProtoThemeToggle;
})(window);
