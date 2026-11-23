const sprite = document.getElementById("sprite");
const stage = document.getElementById("stage");

const tauri = window.__TAURI__;
const DEFAULT_STATE = "idle";
const ERROR_HOLD_MS = 1800;

let manifest = null;
let frames = new Map();
let stateName = DEFAULT_STATE;
let frameIndex = 0;
let timer = null;
let errorTimer = null;

async function loadManifest() {
  const response = await fetch("avatar/manifest.json");
  if (!response.ok) throw new Error(`manifest ${response.status}`);
  return response.json();
}

function preload(states) {
  const loaded = new Map();
  for (const [name, clip] of Object.entries(states)) {
    loaded.set(
      name,
      clip.frames.map((src) => {
        const image = new Image();
        image.src = src;
        return image;
      }),
    );
  }
  return loaded;
}

function paint() {
  const clip = frames.get(stateName) ?? frames.get(DEFAULT_STATE);
  if (!clip?.length) return;
  frameIndex %= clip.length;
  sprite.src = clip[frameIndex].src;
  sprite.dataset.state = stateName;
}

function stop() {
  if (timer) clearInterval(timer);
  timer = null;
}

function play(name) {
  const next = frames.has(name) ? name : DEFAULT_STATE;
  if (next === stateName && timer) return;

  stateName = next;
  frameIndex = 0;
  stop();

  const clip = frames.get(next);
  if (!clip?.length) return;

  paint();
  const fps = Math.max(1, manifest.states[next]?.fps ?? 1);
  if (clip.length > 1) {
    timer = setInterval(() => {
      frameIndex = (frameIndex + 1) % clip.length;
      paint();
    }, Math.round(1000 / fps));
  }
}

function flashError() {
  play("error");
  if (errorTimer) clearTimeout(errorTimer);
  errorTimer = setTimeout(() => play(DEFAULT_STATE), ERROR_HOLD_MS);
}

async function redock() {
  if (!tauri?.core) return;
  try {
    await tauri.core.invoke("dock");
  } catch {
    /* no hyprctl: stay where the window manager put us */
  }
}

async function start() {
  manifest = await loadManifest();
  frames = preload(manifest.states);

  if (tauri?.event) {
    await tauri.event.listen("nerea://state", (event) => {
      const next = event.payload.state;
      if (next === "error") flashError();
      else play(next);
    });
  }

  sprite.addEventListener("error", () => {
    stage.dataset.fallback = "true";
  });

  // Double-click re-seats her on the focused window.
  stage.addEventListener("dblclick", redock);

  // Right-click opens the composer next to her; left drag is left to the WM.
  stage.addEventListener("contextmenu", (event) => {
    event.preventDefault();
    if (tauri?.core) tauri.core.invoke("open_composer");
  });

  await redock();
  play(DEFAULT_STATE);
}

start().catch((error) => {
  console.error("avatar failed to start", error);
  document.body.dataset.error = "true";
});
