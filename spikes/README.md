# spikes/

Throw-away prototypes for the technical spike (decision 7 in docs/spec.md:
Tauri or Electron). Nothing here is product code, and nothing here is part
of the core workspace or CI.

| Folder | Experiment |
| --- | --- |
| `voice/` | 4 – group voice call: browser / Electron (Chromium WebRTC) and native Rust (str0m), Docker bench with netem, Hebrew test guide for friends |
| `shell/` | 6 – footprint: the unchanged `ui/` build packaged in Tauri and in Electron |

Results, the recommendation and exact reproduce commands: [RESULTS.md](RESULTS.md).
