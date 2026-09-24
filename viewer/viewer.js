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

function loadReplay(data) {
  if (!data.ticks || data.ticks.length === 0) throw new Error("replay has no ticks");
  // Replays may come from anyone: colors are only ever used as #rrggbb.
  data.robots.forEach((r, i) => {
    if (typeof r.color !== "string" || !/^#[0-9a-fA-F]{6}$/.test(r.color)) {
      r.color = FALLBACK_COLORS[i % FALLBACK_COLORS.length];
    }
    r.name = String(r.name);
  });
  state.replay = data;
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
    drawTank(r, rep.robots[i].color, scale);
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

function drawTank(r, color, scale) {
  const [x, y] = toCanvas(r[0], r[1]);
  const bodyR = (state.replay.arena.tank_r || 18) * scale;

  // Body: circle with a direction wedge.
  ctx.fillStyle = color;
  ctx.beginPath();
  ctx.arc(x, y, bodyR, 0, Math.PI * 2);
  ctx.fill();
  ctx.strokeStyle = "rgba(0,0,0,.45)";
  ctx.lineWidth = 2;
  ctx.stroke();

  // Direction tick on the body rim.
  const bh = deg2rad(r[2]);
  ctx.strokeStyle = color;
  ctx.lineWidth = 3 * scale;
  ctx.beginPath();
  ctx.moveTo(x + Math.sin(bh) * bodyR * 0.4, y - Math.cos(bh) * bodyR * 0.4);
  ctx.lineTo(x + Math.sin(bh) * bodyR * 0.95, y - Math.cos(bh) * bodyR * 0.95);
  ctx.stroke();

  // Gun: line from center outward.
  const gh = deg2rad(r[3]);
  ctx.strokeStyle = "#e8e2d0";
  ctx.lineWidth = 4 * scale;
  ctx.beginPath();
  ctx.moveTo(x, y);
  ctx.lineTo(x + Math.sin(gh) * bodyR * 1.35, y - Math.cos(gh) * bodyR * 1.35);
  ctx.stroke();

  // Radar: small marker at the rim in the radar direction.
  const rh = deg2rad(r[4]);
  ctx.fillStyle = "#7fd8ff";
  ctx.beginPath();
  ctx.arc(x + Math.sin(rh) * bodyR * 0.75, y - Math.cos(rh) * bodyR * 0.75, 3.5 * scale, 0, Math.PI * 2);
  ctx.fill();
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
  requestAnimationFrame(tick);
}
requestAnimationFrame(tick);

// ---------- utilities ----------

function span(cls, text) {
  const el = document.createElement("span");
  el.className = cls;
  if (text !== undefined) el.textContent = text;
  return el;
}

function hexToRgba(hex, alpha) {
  const n = parseInt(hex.slice(1), 16);
  return `rgba(${(n >> 16) & 255},${(n >> 8) & 255},${n & 255},${alpha})`;
}
