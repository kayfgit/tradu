// Tauri's API, injected as window.__TAURI__ by "withGlobalTauri". Absent when
// the UI runs in a plain browser.
interface Window {
  __TAURI__?: any;
}
