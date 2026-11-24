const tauri = window.__TAURI__;

const log = document.getElementById("log");
const statusLine = document.getElementById("status");
const totals = document.getElementById("totals");
const eraseButton = document.getElementById("erase");
const memoryBlock = document.getElementById("memory");
const memoryText = document.getElementById("memory-text");
const closeButton = document.getElementById("close");
const auditBlock = document.getElementById("audit");
const auditList = document.getElementById("audit-list");

let armErase = null;

function invoke(command, args) {
  return tauri.core.invoke(command, args);
}

function formatTime(at) {
  if (!at) return "";
  const date = new Date(at);
  if (Number.isNaN(date.getTime())) return "";
  return date.toLocaleString("es-AR", {
    day: "2-digit",
    month: "2-digit",
    year: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  });
}

function formatClock(at) {
  if (!at) return "";
  const date = new Date(at);
  if (Number.isNaN(date.getTime())) return "";
  return date.toLocaleTimeString("es-AR", { hour: "2-digit", minute: "2-digit", hour12: false });
}

function describe(error) {
  return typeof error === "string" ? error : (error?.message ?? "algo salió mal");
}

function renderEntry(entry) {
  const article = document.createElement("article");
  article.className = `entry ${entry.role}`;

  const meta = document.createElement("header");
  meta.className = "meta";

  const who = document.createElement("span");
  who.className = "who";
  who.textContent = entry.role === "user" ? "vos" : entry.role === "assistant" ? "nerea" : entry.role;

  const when = document.createElement("span");
  when.className = "when";
  when.textContent = formatTime(entry.at);

  const size = document.createElement("span");
  size.className = "size";
  size.textContent = `~${entry.tokens} tok`;

  meta.append(who, when, size);

  const body = document.createElement("p");
  body.className = "text";
  body.textContent = entry.content;

  article.append(meta, body);
  return article;
}

// What she touched, straight from the audit log. Read-only on purpose: this is
// the panel that makes "she can read my machine" something you can live with.
async function loadAudit() {
  if (!auditBlock || !auditList) return;

  try {
    const calls = await invoke("audit_calls");
    auditList.replaceChildren();
    auditBlock.hidden = calls.length === 0;

    if (calls.length === 0) return;

    const fragment = document.createDocumentFragment();
    for (const call of calls) {
      const item = document.createElement("li");
      item.className = call.ok ? "call" : "call failed";

      const when = document.createElement("span");
      when.className = "call-when";
      when.textContent = formatClock(call.at);

      const tool = document.createElement("span");
      tool.className = "call-tool";
      tool.textContent = call.tool;

      const what = document.createElement("span");
      what.className = "call-what";
      what.textContent = call.summary;

      const size = document.createElement("span");
      size.className = "call-size";
      size.textContent = call.ok ? `${call.bytes} b` : "falló";

      item.append(when, tool, what, size);
      fragment.append(item);
    }

    auditList.append(fragment);
  } catch {
    // The audit is a window into her work, never a reason to break the history.
    auditBlock.hidden = true;
  }
}

async function load() {
  try {
    const data = await invoke("history");
    const model = data.status.model ? ` · ${data.status.model}` : "";

    statusLine.textContent = `${data.status.provider}${model}`;
    statusLine.title = (data.status.warnings ?? []).join("\n");

    const memory = data.status.restored > 0 ? ` · ${data.status.restored} recuperados al abrir` : "";
    totals.textContent =
      `${data.messages} mensajes guardados · ~${data.tokens} tokens en disco · ` +
      `${data.status.turns} en contexto · ${data.status.memory_words} palabras de memoria${memory}`;

    await loadAudit();
    log.replaceChildren();

    if (data.entries.length === 0) {
      const empty = document.createElement("p");
      empty.className = "empty";
      empty.textContent = "Todavía no hay nada guardado.";
      log.append(empty);
      return;
    }

    const summary = data.memory?.trim() ?? "";
    memoryBlock.hidden = summary.length === 0;
    memoryText.textContent = summary;

    const fragment = document.createDocumentFragment();
    for (const entry of data.entries) fragment.append(renderEntry(entry));
    log.append(fragment);
    log.scrollTop = log.scrollHeight;
  } catch (error) {
    statusLine.textContent = `error: ${describe(error)}`;
  }
}

async function start() {
  if (!tauri?.core) {
    statusLine.textContent = "abrí esta ventana desde Nerea";
    return;
  }

  eraseButton.addEventListener("click", async () => {
    if (!armErase) {
      armErase = setTimeout(() => {
        armErase = null;
        eraseButton.textContent = "borrar memoria";
      }, 4000);
      eraseButton.textContent = "¿borrar todo? tocá otra vez";
      return;
    }

    clearTimeout(armErase);
    armErase = null;
    eraseButton.textContent = "borrar memoria";

    try {
      await invoke("reset_conversation");
    } catch (error) {
      statusLine.textContent = `error: ${describe(error)}`;
    }
    await load();
  });

  closeButton.addEventListener("click", () => invoke("close_history").catch(() => {}));

  // Reopening the window must show the newest turns, not the ones from before.
  window.addEventListener("focus", load);

  await load();
}

start();
