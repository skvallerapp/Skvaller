// Gives a window's page its theme before the page first paints, as `styleRoot` in
// theme.ts does once the settings have loaded. Without it a window opens in the light
// palette and switches when the settings arrive. See `Theme::script` in config.rs, which
// sets `window.__SKVALLER_THEME__` to the `theme` setting just before this runs.
(function () {
  // Windows runs these scripts in frames too. The article frame styles itself.
  if (window.top !== window) return
  var theme = window.__SKVALLER_THEME__
  // System is Light or Dark as the webview says, which is what `palette` in theme.ts asks.
  if (theme === 'system') {
    theme = matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'light'
  }
  var apply = function () {
    document.documentElement.dataset.theme = theme
  }
  if (document.documentElement) {
    apply()
  } else {
    // WebView2 can run the script before the document has a root element.
    new MutationObserver(function (_, observer) {
      if (document.documentElement) {
        observer.disconnect()
        apply()
      }
    }).observe(document, { childList: true })
  }
})()
