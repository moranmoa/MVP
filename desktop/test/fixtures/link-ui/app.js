// Stand-in UI for test/deeplink.e2e.test.mjs: subscribes to the bridge a
// second after load, like a UI that mounts late, and records every
// link.open. The real UI is not used so its own listener cannot race the test.
window.__links = [];
window.__status = [];
setTimeout(() => {
  window.mvs.onEvent((name, payload) => {
    if (name === 'link.open') window.__links.push(payload);
    if (name === 'core.status') window.__status.push(payload);
  });
  window.__subscribed = true;
}, 1000);
