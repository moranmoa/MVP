// Signalling server for the voice spike (experiment 4).
//
// One WebSocket per participant. It only relays SDP between peers of the
// same room; audio never passes through it. It also serves the browser /
// Electron client from ../web and stores the reports peers send at the end
// of a call in $RESULTS_DIR.
//
// Protocol (JSON text frames):
//   -> {type:'join', room, id, kind}       kind: 'chromium' | 'str0m' | 'webkit' | ...
//   <- {type:'peers', peers:[{id, kind}]}   who is already in the room
//   <- {type:'joined', id, kind} / {type:'left', id}
//   -> {type:'signal', to, data}           relayed as {type:'signal', from, data}
//   -> {type:'report', data}               written to $RESULTS_DIR/<room>/<id>.json
//
// SDP is sent complete (no trickle ICE): both the browser and the str0m peer
// put their candidates in the SDP, which keeps the str0m side simple.

import { createServer } from 'node:http';
import { mkdirSync, readFileSync, writeFileSync, existsSync } from 'node:fs';
import { extname, join, normalize } from 'node:path';
import { fileURLToPath } from 'node:url';
import { WebSocketServer } from 'ws';

const PORT = Number(process.env.PORT || 8080);
const WEB_DIR = fileURLToPath(new URL('../web/', import.meta.url));
const RESULTS_DIR = process.env.RESULTS_DIR || '';
const TYPES = { '.html': 'text/html; charset=utf-8', '.js': 'text/javascript', '.css': 'text/css', '.json': 'application/json' };

const http = createServer((req, res) => {
  const url = new URL(req.url, 'http://x');
  // ICE servers come from the environment so no address is ever committed.
  if (url.pathname === '/config.json') {
    res.writeHead(200, { 'content-type': TYPES['.json'], 'access-control-allow-origin': '*' });
    res.end(JSON.stringify({ iceServers: iceServers() }));
    return;
  }
  const path = normalize(join(WEB_DIR, url.pathname === '/' ? 'index.html' : url.pathname));
  if (!path.startsWith(normalize(WEB_DIR)) || !existsSync(path)) {
    res.writeHead(404).end();
    return;
  }
  res.writeHead(200, { 'content-type': TYPES[extname(path)] || 'application/octet-stream' });
  res.end(readFileSync(path));
});

/** STUN_URLS="stun:host:3478", optional TURN_URLS + TURN_USER + TURN_PASS. */
function iceServers() {
  const list = [];
  if (process.env.STUN_URLS) list.push({ urls: process.env.STUN_URLS.split(',') });
  if (process.env.TURN_URLS) {
    list.push({ urls: process.env.TURN_URLS.split(','), username: process.env.TURN_USER, credential: process.env.TURN_PASS });
  }
  return list;
}

const rooms = new Map(); // room -> Map(id -> {ws, kind})

const wss = new WebSocketServer({ server: http });
wss.on('connection', (ws) => {
  let room = null;
  let id = null;
  ws.on('message', (raw) => {
    let msg;
    try {
      msg = JSON.parse(raw);
    } catch {
      return;
    }
    if (msg.type === 'join' && !room) {
      room = String(msg.room);
      id = String(msg.id);
      if (!rooms.has(room)) rooms.set(room, new Map());
      const members = rooms.get(room);
      ws.send(JSON.stringify({ type: 'peers', peers: [...members].map(([pid, m]) => ({ id: pid, kind: m.kind })) }));
      for (const m of members.values()) m.ws.send(JSON.stringify({ type: 'joined', id, kind: msg.kind }));
      members.set(id, { ws, kind: msg.kind });
      console.log(`join ${room}/${id} (${msg.kind}), ${members.size} in room`);
    } else if (msg.type === 'signal' && room) {
      rooms.get(room)?.get(msg.to)?.ws.send(JSON.stringify({ type: 'signal', from: id, data: msg.data }));
    } else if (msg.type === 'report' && room && RESULTS_DIR) {
      const dir = join(RESULTS_DIR, room.replace(/[^\w.-]/g, '_'));
      mkdirSync(dir, { recursive: true });
      writeFileSync(join(dir, `${id.replace(/[^\w.-]/g, '_')}.json`), JSON.stringify(msg.data, null, 2));
      console.log(`report ${room}/${id}`);
    }
  });
  ws.on('close', () => {
    const members = room && rooms.get(room);
    if (!members) return;
    members.delete(id);
    for (const m of members.values()) m.ws.send(JSON.stringify({ type: 'left', id }));
    if (members.size === 0) rooms.delete(room);
  });
});

http.listen(PORT, () => console.log(`signal on :${PORT}, web from ${WEB_DIR}`));
