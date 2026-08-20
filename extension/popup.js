// Popup UI: shows the current tab and forwards an "add" request to the
// background script, which owns the native-messaging connection.
const api = (typeof browser !== "undefined") ? browser : chrome;

const urlEl = document.getElementById("url");
const addEl = document.getElementById("add");
const forceEl = document.getElementById("force");
const statusEl = document.getElementById("status");

let currentUrl = "";

async function init() {
  const tabs = await api.tabs.query({ active: true, currentWindow: true });
  currentUrl = (tabs && tabs[0] && tabs[0].url) || "";
  urlEl.textContent = currentUrl || "No active tab";
  addEl.disabled = !currentUrl;
}

function setStatus(text, kind) {
  statusEl.textContent = text;
  statusEl.className = "status" + (kind ? " " + kind : "");
}

addEl.addEventListener("click", async () => {
  if (!currentUrl) return;
  addEl.disabled = true;
  setStatus("Sending to Grimoire…");
  try {
    const result = await api.runtime.sendMessage({
      type: "add",
      url: currentUrl,
      force: forceEl.checked,
    });
    setStatus(result.message, result.ok ? "ok" : "err");
  } catch (error) {
    setStatus("Failed: " + (error && error.message ? error.message : error), "err");
  } finally {
    addEl.disabled = false;
  }
});

init();
