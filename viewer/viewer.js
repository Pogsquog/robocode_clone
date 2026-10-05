// tank replay viewer: plays back the JSON emitted by `tank run`.
// Plain browser JS, no build step, no server; loads replays from a file
// picker or drag-and-drop (works from file:// with no CORS issues).

"use strict";

const BASE_TPS = 30; // ticks per second at 1x

const state = {
  replay: null,   // parsed replay
  frame: 0,       // current tick index into replay.ticks
  playing: false,
  speed: 1,
  lastFrameTime: null,
  acc: 0,
  treads: null,   // per-robot cumulative track travel, see computeTreads
};

const FALLBACK_COLORS = ["#e74c3c", "#3498db", "#2ecc71", "#f39c12", "#9b59b6", "#1abc9c", "#e91e63", "#cddc39"];

const canvas = document.getElementById("arena");
const ctx = canvas.getContext("2d");
const playBtn = document.getElementById("playBtn");
const scrub = document.getElementById("scrub");
const tickLabel = document.getElementById("tickLabel");
const logwrap = document.getElementById("logwrap");
const banner = document.getElementById("banner");
const hint = document.getElementById("hint");
const bars = document.getElementById("energybars");

// ---------- loading ----------

document.getElementById("fileInput").addEventListener("change", (e) => {
  const f = e.target.files[0];
  if (f) loadFile(f);
});

window.addEventListener("dragover", (e) => e.preventDefault());
window.addEventListener("drop", (e) => {
  e.preventDefault();
  const f = e.dataTransfer.files[0];
  if (f) loadFile(f);
});

function loadFile(f) {
  const reader = new FileReader();
  reader.onload = () => {
    try {
      const data = JSON.parse(reader.result);
      if (data.format !== "tank-replay") throw new Error("not a tank replay file");
      loadReplay(data);
    } catch (err) {
      alert("could not load replay: " + err.message);
    }
  };
  reader.readAsText(f);
}

// Replays may come from anyone. Check everything drawing relies on before
// touching any state, so a bad file is rejected and the current replay (and
// the animation loop) carry on untouched. Size limits bound drawing work.
function validateReplay(data) {
  const fail = (msg) => { throw new Error(msg); };
  const isNum = (v, lo, hi) => typeof v === "number" && Number.isFinite(v) && v >= lo && v <= hi;
  const isIndex = (v, n) => Number.isInteger(v) && v >= 0 && v < n;
  const LIMIT = 1e6; // any coordinate, angle or energy

  const a = data.arena;
  if (!a || typeof a !== "object") fail("missing arena");
  if (!isNum(a.w, 100, 20000) || !isNum(a.h, 100, 20000)) fail("bad arena size");
  if (a.tank_r !== undefined && !isNum(a.tank_r, 1, 200)) fail("bad tank radius");
  if (a.beam !== undefined && !isNum(a.beam, 0, 360)) fail("bad radar beam");

  const n = Array.isArray(data.robots) ? data.robots.length : 0;
  if (n < 1 || n > 256) fail("replay must have 1 to 256 robots");
  if (!data.robots.every((r) => r && typeof r === "object")) fail("bad robot entry");

  if (!Array.isArray(data.ticks) || data.ticks.length === 0) fail("replay has no ticks");
  data.ticks.forEach((t, k) => {
    const where = `tick ${k}`;
    if (!t || typeof t !== "object" || !isNum(t.t, 0, Number.MAX_SAFE_INTEGER)) fail(`${where}: bad tick`);
    if (!Array.isArray(t.r) || t.r.length !== n) fail(`${where}: expected ${n} robots`);
    for (const r of t.r) {
      if (!Array.isArray(r) || r.length !== 7 || !r.every((v) => isNum(v, -LIMIT, LIMIT))) {
        fail(`${where}: bad robot state`);
      }
    }
    if (t.b !== undefined) {
      if (!Array.isArray(t.b)) fail(`${where}: bad bullets`);
      for (const b of t.b) {
        if (!Array.isArray(b) || b.length !== 4 || !isNum(b[0], -LIMIT, LIMIT) ||
            !isNum(b[1], -LIMIT, LIMIT) || !isIndex(b[2], n) || !isNum(b[3], 0, 10)) {
          fail(`${where}: bad bullet`);
        }
      }
    }
    if (t.e !== undefined && !Array.isArray(t.e)) fail(`${where}: bad events`);
  });

  if (data.result !== undefined && data.result !== null) {
    const w = data.result.winner;
    if (w !== null && w !== undefined && !isIndex(w, n)) fail("bad winner");
  }
}

function loadReplay(data) {
  validateReplay(data);
  data.ticks.forEach((t) => { if (t.e) t.e = t.e.map(String); });
  if (data.result) data.result.reason = String(data.result.reason ?? "");
  // Colors are only ever used as #rrggbb.
  data.robots.forEach((r, i) => {
    if (typeof r.color !== "string" || !/^#[0-9a-fA-F]{6}$/.test(r.color)) {
      r.color = FALLBACK_COLORS[i % FALLBACK_COLORS.length];
    }
    r.name = String(r.name);
  });
  state.replay = data;
  state.treads = computeTreads(data);
  state.frame = 0;
  state.playing = false;
  logwrap.replaceChildren();
  absorbEvents(0, logwrap);
  banner.style.display = "none";
  hint.style.display = "none";

  canvas.dataset.w = data.arena.w;
  canvas.dataset.h = data.arena.h;

  // Energy bars (built with DOM APIs: nothing from the file becomes markup).
  bars.replaceChildren();
  data.robots.forEach((r, i) => {
    const bar = document.createElement("div");
    bar.className = "bar";
    const name = span("name", r.name);
    name.style.color = r.color;
    const track = span("track");
    const fill = span("fill");
    fill.id = "fill" + i;
    fill.style.background = r.color;
    fill.style.width = "100%";
    track.appendChild(fill);
    const val = span("val", "100");
    val.id = "val" + i;
    bar.append(name, track, val);
    bars.appendChild(bar);
  });

  scrub.max = data.ticks.length - 1;
  scrub.value = 0;
  scrub.disabled = false;
  playBtn.disabled = false;
  document.getElementById("backBtn").disabled = false;
  document.getElementById("fwdBtn").disabled = false;
  fitCanvas(); // after the energy bars exist: they take vertical space
  setPlaying(true);
}

// Scale the canvas to the largest size that fits inside #stage (which flex
// layout sizes to whatever the header, bars, controls and log leave over).
function fitCanvas() {
  if (!state.replay) return;
  const w = state.replay.arena.w, h = state.replay.arena.h;
  const stage = document.getElementById("stage");
  const cs = getComputedStyle(stage);
  const border = 4; // #arena has a 2px border on each side
  const availW = stage.clientWidth - parseFloat(cs.paddingLeft) - parseFloat(cs.paddingRight) - border;
  const availH = stage.clientHeight - parseFloat(cs.paddingTop) - parseFloat(cs.paddingBottom) - border;
  const scale = Math.max(0.05, Math.min(availW / w, availH / h));
  canvas.width = Math.floor(w * scale);
  canvas.height = Math.floor(h * scale);
  draw();
}

window.addEventListener("resize", fitCanvas);

// ---------- playback ----------

function setPlaying(on) {
  state.playing = on && !!state.replay;
  playBtn.textContent = state.playing ? "Pause" : "Play";
  if (state.playing) state.lastFrameTime = null;
}

playBtn.addEventListener("click", () => setPlaying(!state.playing));
document.getElementById("speed").addEventListener("change", (e) => {
  state.speed = parseFloat(e.target.value);
});
document.getElementById("backBtn").addEventListener("click", () => {
  setFrame(Math.max(0, state.frame - 30));
});
document.getElementById("fwdBtn").addEventListener("click", () => {
  const max = state.replay ? state.replay.ticks.length - 1 : 0;
  setFrame(Math.min(max, state.frame + 30));
});
scrub.addEventListener("input", () => {
  setFrame(parseInt(scrub.value, 10));
  setPlaying(false);
});
window.addEventListener("keydown", (e) => {
  if (!state.replay) return;
  if (e.code === "Space") { e.preventDefault(); setPlaying(!state.playing); }
  else if (e.code === "ArrowLeft") setFrame(Math.max(0, state.frame - 1));
  else if (e.code === "ArrowRight") {
    setFrame(Math.min(state.replay.ticks.length - 1, state.frame + 1));
  }
});

function setFrame(n) {
  if (!state.replay) return;
  // Rebuild the log when scrubbing backwards; otherwise append. Either way,
  // build off-DOM and attach once.
  const frag = document.createDocumentFragment();
  if (n < state.frame) {
    for (let i = 0; i <= n; i++) absorbEvents(i, frag);
    logwrap.replaceChildren(frag);
  } else {
    for (let i = state.frame + 1; i <= n; i++) absorbEvents(i, frag);
    logwrap.appendChild(frag);
  }
  logwrap.scrollTop = logwrap.scrollHeight;
  state.frame = n;
  scrub.value = n;
  draw();
}

// Append tick `idx`'s events to `target` as log lines.
function absorbEvents(idx, target) {
  const t = state.replay.ticks[idx];
  if (!t.e || t.e.length === 0) return;
  for (const ev of t.e) {
    const cls = ev.includes("destroyed") || ev.includes("forfeit")
      ? "death"
      : ev.includes("bullet hit") ? "hit" : "log";
    const div = document.createElement("div");
    div.className = "ev " + cls;
    div.textContent = `[${String(t.t).padStart(4)}] ${ev}`;
    target.appendChild(div);
  }
}

// ---------- rendering ----------

function toCanvas(x, y) {
  const w = parseFloat(canvas.dataset.w), h = parseFloat(canvas.dataset.h);
  return [ (x / w) * canvas.width, (y / h) * canvas.height ];
}

function deg2rad(d) { return d * Math.PI / 180; }

function draw() {
  const rep = state.replay;
  if (!rep) return;
  const snap = rep.ticks[state.frame];
  const W = canvas.width, H = canvas.height;
  const scale = W / parseFloat(canvas.dataset.w);

  // Arena floor with a faint grid.
  ctx.fillStyle = "#0d0f13";
  ctx.fillRect(0, 0, W, H);
  ctx.strokeStyle = "#1a1f27";
  ctx.lineWidth = 1;
  const grid = 50 * scale;
  for (let gx = grid; gx < W; gx += grid) {
    ctx.beginPath(); ctx.moveTo(gx, 0); ctx.lineTo(gx, H); ctx.stroke();
  }
  for (let gy = grid; gy < H; gy += grid) {
    ctx.beginPath(); ctx.moveTo(0, gy); ctx.lineTo(W, gy); ctx.stroke();
  }

  // Radar beams (before tanks so tanks draw on top).
  snap.r.forEach((r, i) => {
    if (r[6] < 0.5) return;
    drawBeam(r, i, scale);
  });

  // Bullets.
  if (snap.b) {
    for (const b of snap.b) {
      const [bx, by] = toCanvas(b[0], b[1]);
      const rad = (2 + b[3]) * scale;
      ctx.fillStyle = rep.robots[b[2]].color;
      ctx.beginPath();
      ctx.arc(bx, by, rad, 0, Math.PI * 2);
      ctx.fill();
      ctx.strokeStyle = "rgba(255,255,255,.5)";
      ctx.lineWidth = 1;
      ctx.stroke();
    }
  }

  // Tanks.
  snap.r.forEach((r, i) => {
    if (r[6] < 0.5) return;
    drawTank(r, i, rep.robots[i].color, scale);
  });

  // Energy bars + labels.
  snap.r.forEach((r, i) => {
    const e = Math.max(0, r[5]);
    const fill = document.getElementById("fill" + i);
    const val = document.getElementById("val" + i);
    if (fill) fill.style.width = Math.min(100, e) + "%";
    if (val) val.textContent = e.toFixed(0);
  });

  tickLabel.textContent = `tick ${snap.t} / ${rep.ticks[rep.ticks.length - 1].t}`;

  // Result banner on the last frame.
  const last = state.frame === rep.ticks.length - 1;
  if (last && rep.result) {
    const wnr = rep.result.winner;
    banner.textContent = wnr === null || wnr === undefined
      ? "draw"
      : `winner: ${rep.robots[wnr].name} (${rep.result.reason.replace("_", " ")})`;
    banner.style.display = "block";
    setPlaying(false);
  } else {
    banner.style.display = "none";
  }
}

// Tank drawing proportions, as fractions of the collision radius. The hull
// is slightly narrower than it is long; the tracks run down either side.
const TANK = {
  len: 1.9,       // overall length (tracks)
  wid: 1.7,       // overall width across both tracks
  track: 0.42,    // width of each track
  hullLen: 1.6,   // hull length between the tracks
  cleat: 4,       // tread cleat spacing, in arena units
};

function drawTank(r, i, color, scale) {
  const [x, y] = toCanvas(r[0], r[1]);
  const R = (state.replay.arena.tank_r || 18) * scale;
  const L = TANK.len * R, Wd = TANK.wid * R, tw = TANK.track * R;
  const tread = state.treads[i];
  const f = state.frame * 2;

  // Body frame: rotate so local -y is the body's forward direction.
  ctx.save();
  ctx.translate(x, y);
  ctx.rotate(deg2rad(r[2]));

  drawTrack(-Wd / 2, -L / 2, tw, L, tread[f], scale);
  drawTrack(Wd / 2 - tw, -L / 2, tw, L, tread[f + 1], scale);

  // Hull between the tracks, with a darker front glacis showing heading.
  const hw = Wd - 2 * tw + 2 * scale, hl = TANK.hullLen * R;
  ctx.fillStyle = color;
  ctx.fillRect(-hw / 2, -hl / 2, hw, hl);
  ctx.fillStyle = "rgba(0,0,0,.28)";
  ctx.fillRect(-hw / 2, -hl / 2, hw, hl * 0.18);
  ctx.strokeStyle = "rgba(0,0,0,.55)";
  ctx.lineWidth = Math.max(1, 1.5 * scale);
  ctx.strokeRect(-hw / 2, -hl / 2, hw, hl);
  ctx.restore();

  // Turret and barrel, in the gun's frame.
  ctx.save();
  ctx.translate(x, y);
  ctx.rotate(deg2rad(r[3]));
  const bw = 3.5 * scale;
  ctx.fillStyle = "#c9c3b0";
  ctx.fillRect(-bw / 2, -R * 1.45, bw, R * 1.45);
  ctx.strokeStyle = "rgba(0,0,0,.6)";
  ctx.lineWidth = 1;
  ctx.strokeRect(-bw / 2, -R * 1.45, bw, R * 1.45);
  ctx.fillStyle = shade(color, 0.72);
  roundRect(-R * 0.5, -R * 0.55, R, R * 1.05, R * 0.25);
  ctx.fill();
  ctx.strokeStyle = "rgba(0,0,0,.6)";
  ctx.lineWidth = Math.max(1, 1.5 * scale);
  ctx.stroke();
  ctx.restore();

  // Radar: small dish on the turret facing the radar direction.
  const rh = deg2rad(r[4]);
  ctx.save();
  ctx.translate(x, y);
  ctx.rotate(rh);
  ctx.strokeStyle = "#7fd8ff";
  ctx.lineWidth = Math.max(1.5, 2.2 * scale);
  ctx.beginPath();
  ctx.arc(0, R * 0.35, R * 0.32, deg2rad(-150), deg2rad(-30));
  ctx.stroke();
  ctx.fillStyle = "#7fd8ff";
  ctx.beginPath();
  ctx.arc(0, R * 0.2, 1.8 * scale, 0, Math.PI * 2);
  ctx.fill();
  ctx.restore();
}

// One track, in body-local coordinates. `travel` is how far this side has
// rolled (arena units); cleats scroll with it so the tread visibly moves.
function drawTrack(x0, y0, w, h, travel, scale) {
  ctx.fillStyle = "#23272d";
  ctx.fillRect(x0, y0, w, h);
  const step = TANK.cleat * scale;
  // The top run of a track moves forward relative to the hull, i.e. toward -y.
  let off = -((travel * scale) % step);
  if (off < 0) off += step;
  ctx.save();
  ctx.beginPath();
  ctx.rect(x0, y0, w, h);
  ctx.clip();
  ctx.strokeStyle = "#5b636e";
  ctx.lineWidth = Math.max(1, 1.4 * scale);
  ctx.beginPath();
  for (let cy = y0 + off - step; cy < y0 + h + step; cy += step) {
    ctx.moveTo(x0 + 1, cy);
    ctx.lineTo(x0 + w - 1, cy);
  }
  ctx.stroke();
  ctx.restore();
  ctx.strokeStyle = "#0b0c0e";
  ctx.lineWidth = 1;
  ctx.strokeRect(x0, y0, w, h);
}

// Cumulative distance rolled by each track, for every frame of every robot:
// treads[robot][2*frame] = left, [2*frame+1] = right. Derived from the pose
// history so the animation is right when scrubbing, and turning in place
// runs the two tracks in opposite directions.
function computeTreads(rep) {
  const n = rep.ticks.length;
  const halfW = (rep.arena.tank_r || 18) * (TANK.wid - TANK.track) / 2;
  return rep.robots.map((_, i) => {
    const out = new Float64Array(n * 2);
    for (let k = 1; k < n; k++) {
      const a = rep.ticks[k - 1].r[i], b = rep.ticks[k].r[i];
      let dl = 0, dr = 0;
      if (a && b && a[6] >= 0.5 && b[6] >= 0.5) {
        const h = deg2rad(a[2]);
        const fwd = (b[0] - a[0]) * Math.sin(h) - (b[1] - a[1]) * Math.cos(h);
        let dh = b[2] - a[2];
        dh -= 360 * Math.round(dh / 360);
        // Clockwise turn: the left track runs forward, the right backward.
        dl = fwd + deg2rad(dh) * halfW;
        dr = fwd - deg2rad(dh) * halfW;
      }
      out[2 * k] = out[2 * k - 2] + dl;
      out[2 * k + 1] = out[2 * k - 1] + dr;
    }
    return out;
  });
}

function drawBeam(r, robotIndex, scale) {
  const [x, y] = toCanvas(r[0], r[1]);
  const half = deg2rad((state.replay.arena.beam || 18) / 2);
  // Canvas arc angles start at +x (east); game headings start at north.
  const heading = deg2rad(r[4] - 90);
  const len = canvas.width + canvas.height;
  const color = state.replay.robots[robotIndex].color;
  ctx.fillStyle = hexToRgba(color, 0.10);
  ctx.beginPath();
  ctx.moveTo(x, y);
  ctx.arc(x, y, len, heading - half, heading + half);
  ctx.closePath();
  ctx.fill();
}

// ---------- main loop ----------

function tick(now) {
  // Schedule the next frame first, so an error drawing this one can never
  // stop the loop for good.
  requestAnimationFrame(tick);
  try {
    advance(now);
  } catch (err) {
    setPlaying(false);
    console.error("playback stopped:", err);
  }
}

function advance(now) {
  if (state.playing && state.replay) {
    if (state.lastFrameTime === null) state.lastFrameTime = now;
    const dt = (now - state.lastFrameTime) / 1000;
    state.lastFrameTime = now;
    state.acc += dt * BASE_TPS * state.speed;
    while (state.acc >= 1) {
      state.acc -= 1;
      const max = state.replay.ticks.length - 1;
      if (state.frame < max) {
        setFrame(state.frame + 1);
      } else {
        setPlaying(false);
        break;
      }
    }
  } else {
    state.lastFrameTime = null;
    state.acc = 0;
  }
}
requestAnimationFrame(tick);

// ---------- utilities ----------

function span(cls, text) {
  const el = document.createElement("span");
  el.className = cls;
  if (text !== undefined) el.textContent = text;
  return el;
}

// Scale a #rrggbb color's channels by `k` (<1 darkens).
function shade(hex, k) {
  const n = parseInt(hex.slice(1), 16);
  const c = (v) => Math.round(Math.min(255, v * k));
  return `rgb(${c((n >> 16) & 255)},${c((n >> 8) & 255)},${c(n & 255)})`;
}

function roundRect(x, y, w, h, rad) {
  ctx.beginPath();
  ctx.moveTo(x + rad, y);
  ctx.arcTo(x + w, y, x + w, y + h, rad);
  ctx.arcTo(x + w, y + h, x, y + h, rad);
  ctx.arcTo(x, y + h, x, y, rad);
  ctx.arcTo(x, y, x + w, y, rad);
  ctx.closePath();
}

function hexToRgba(hex, alpha) {
  const n = parseInt(hex.slice(1), 16);
  return `rgba(${(n >> 16) & 255},${(n >> 8) & 255},${n & 255},${alpha})`;
}
