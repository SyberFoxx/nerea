const log = document.getElementById("log");
const input = document.getElementById("input");
const form = document.getElementById("composer");
const sendButton = document.getElementById("send");
const stopButton = document.getElementById("stop");
const statusLine = document.getElementById("status");
const resetButton = document.getElementById("reset");

const tauri = window.__TAURI__;
let streamingBody = null;
let busy = false;

function addBubble(role, text = "") {
  log.querySelector(".empty")?.remove();

  const article = document.createElement("article");
  article.className = `bubble ${role}`;

  const body = document.createElement("p");
  body.textContent = text;
  article.append(body);
  log.append(article);

  scrollToEnd();
  return body;
}

function scrollToEnd() {
  log.scrollTop = log.scrollHeight;
}

function setBusy(value) {
  busy = value;
  sendButton.disabled = value;
  stopButton.hidden = !value;
  input.disabled = value;
  if (!value) input.focus();
}

async function refreshStatus() {
  try {
    const info = await tauri.core.invoke("status");
    const model = info.model ? ` · ${info.model}` : "";
    statusLine.textContent = `${info.provider}${model} · ${info.turns} turns`;
    statusLine.title = (info.warnings ?? []).join("\n");
  } catch (error) {
    statusLine.textContent = `error: ${error}`;
  }
}

function describe(error) {
  return typeof error === "string" ? error : (error?.message ?? "unexpected error");
}

async function send(text) {
  addBubble("user", text);
  streamingBody = addBubble("nerea");
  setBusy(true);

  try {
    await tauri.core.invoke("send_message", { text });
  } catch (error) {
    streamingBody.textContent = `⚠ ${describe(error)}`;
    streamingBody.parentElement.classList.add("failed");
  } finally {
    streamingBody = null;
    setBusy(false);
    await refreshStatus();
  }
}

async function start() {
  if (!tauri?.core) {
    statusLine.textContent = "not running inside tauri";
    addBubble("nerea", "This window must be opened through the nerea desktop app, not a browser.");
    return;
  }

  await tauri.event.listen("nerea://delta", (event) => {
    if (!streamingBody) return;
    streamingBody.textContent += event.payload.text;
    scrollToEnd();
  });

  form.addEventListener("submit", (event) => {
    event.preventDefault();
    const text = input.value.trim();
    if (!text || busy) return;
    input.value = "";
    send(text);
  });

  input.addEventListener("keydown", (event) => {
    if (event.key === "Enter" && !event.shiftKey) {
      event.preventDefault();
      form.requestSubmit();
    }
  });

  stopButton.addEventListener("click", () => tauri.core.invoke("cancel_stream"));

  resetButton.addEventListener("click", async () => {
    if (busy) return;
    await tauri.core.invoke("reset_conversation");
    log.replaceChildren();
    log.append(Object.assign(document.createElement("p"), { className: "empty", textContent: "Nothing yet. Write something below." }));
    await refreshStatus();
  });

  await refreshStatus();
  input.focus();
}

start();
