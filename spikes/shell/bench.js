// Footprint bench (experiment 6), injected by both shells when
// SHELL_BENCH_OUT is set. Same script in Tauri and Electron.
//
// Marks (written by the shell as "<label> <epoch ms>" lines):
//   rendered  the demo channel shows its first message rows (cold start ends)
//   scrolled  10,000 messages of the 100k demo channel have been scrolled
//             through (rows fetched from the mock core and rendered)
(async () => {
  const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
  const mark = (label) =>
    window.__TAURI_INTERNALS__
      ? window.__TAURI_INTERNALS__.invoke('bench_mark', { label })
      : console.log(`BENCH_MARK ${label}`);

  while (!document.querySelector('.message-scroll [data-index]')) await sleep(5);
  mark('rendered');

  // Leave time for the harness to sample idle memory first.
  await sleep(15000);
  const el = document.querySelector('.message-scroll');
  const top = () => Math.min(...[...el.querySelectorAll('[data-index]')].map((r) => Number(r.dataset.index)));
  const start = top();
  while (start - top() < 10000 && el.scrollTop > 0) {
    el.scrollTop = Math.max(0, el.scrollTop - el.clientHeight * 3);
    await sleep(16);
  }
  await sleep(2000);
  mark(`scrolled`);
})();
