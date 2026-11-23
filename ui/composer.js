const tauri = window.__TAURI__;
const form = document.getElementById("composer");
const input = document.getElementById("prompt");
const bubble = document.getElementById("bubble");
const sendButton = document.getElementById("send");

let pending = false;

function show(text) {
  bubble.hidden = false;
  bubble.textContent = text;
  bubble.scrollTop = bubble.scrollHeight;
}

function reset() {
  bubble.hidden = true;
  bubble.textContent = "";
}

function close() {
  if (pending) return;
  input.value = "";
  reset();
  tauri.core.invoke("close_composer");
}

async function submit(text) {
  pending = true;
  sendButton.disabled = true;
  bubble.dataset.state = "pending";
  reset();

  try {
    await tauri.core.invoke("send_message", { text });
  } catch (error) {
    const message = typeof error === "string" ? error : (error?.message ?? "unexpected error");
    show(`⚠ ${message}`);
  } finally {
    pending = false;
    sendButton.disabled = false;
    input.focus();
  }
}

async function start() {
  if (!tauri?.core) {
    show("This window only works inside the nerea app.");
    return;
  }

  await tauri.event.listen("nerea://delta", (event) => {
    if (!pending) return;
    if (bubble.hidden) bubble.hidden = false;
    bubble.textContent += event.payload.text;
    bubble.scrollTop = bubble.scrollHeight;
  });

  form.addEventListener("submit", (event) => {
    event.preventDefault();
    const text = input.value.trim();
    if (!text || pending) return;
    input.value = "";
    submit(text);
  });

  input.addEventListener("keydown", (event) => {
    if (event.key === "Escape") close();
  });

  window.addEventListener("focus", () => input.focus());
  input.focus();
}

start();
