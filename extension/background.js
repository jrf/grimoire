// Grimoire browser extension — background script.
//
// Turns "the tab I'm looking at" into an input Grimoire's importer understands
// (arXiv ID, DOI, PubMed ID, or a direct PDF URL) and hands it to the local
// `grimoire browser-host` process over native messaging. Nothing is uploaded to
// any server; the request goes straight to the binary on your machine.

// Firefox/Safari expose promise-based `browser.*`; Chrome/Edge/Brave expose
// `chrome.*`. One alias keeps the logic identical across engines.
const api = (typeof browser !== "undefined") ? browser : chrome;

// Must match `HOST_NAME` in src/browser_host.rs and the installed manifest.
const HOST = "com.grimoire.host";

// Reduce a page URL to the most specific identifier Grimoire can import. The CLI
// already recognizes all of these, so we mostly pass the URL through and only
// special-case a few hosts where the bare ID is cleaner/more reliable.
function resolveInput(rawUrl) {
  let url;
  try {
    url = new URL(rawUrl);
  } catch {
    return rawUrl;
  }
  const host = url.hostname.replace(/^www\./, "");
  const path = url.pathname;

  // arXiv: /abs/1706.03762, /pdf/1706.03762v3 → bare ID
  if (host.endsWith("arxiv.org")) {
    const m = path.match(/\/(?:abs|pdf)\/([0-9]{4}\.[0-9]{4,5})(v[0-9]+)?/);
    if (m) return m[1];
  }
  // doi.org / dx.doi.org → the DOI itself (grimoire prefers content negotiation)
  if (host === "doi.org" || host === "dx.doi.org") {
    return decodeURIComponent(path.replace(/^\//, ""));
  }
  // PubMed: /12345678  → PMID; PMC: /articles/PMC123456/ → PMCID
  if (host === "pubmed.ncbi.nlm.nih.gov") {
    const m = path.match(/\/(\d+)/);
    if (m) return m[1];
  }
  if (host === "pmc.ncbi.nlm.nih.gov" || host.endsWith("ncbi.nlm.nih.gov")) {
    const m = path.match(/PMC(\d+)/i);
    if (m) return "PMC" + m[1];
  }
  // A DOI embedded anywhere in the path (e.g. publisher landing pages).
  const doi = rawUrl.match(/10\.\d{4,9}\/[^\s"'<>?#]+/);
  if (doi) return doi[0];

  // Otherwise hand the full URL to grimoire, which scrapes citation meta tags
  // and falls back to treating it as a direct PDF.
  return rawUrl;
}

// Send one request to the native host and normalize the reply into
// { ok, message } for the caller (popup/notification).
async function sendToGrimoire(input, force = false) {
  const request = { action: "add", input, force };
  let response;
  try {
    response = await api.runtime.sendNativeMessage(HOST, request);
  } catch (error) {
    return {
      ok: false,
      message:
        "Could not reach Grimoire. Install the native host with " +
        "`grimoire install-browser-host --extension-id <id>` and confirm the " +
        "binary is on your PATH. (" + (error && error.message ? error.message : error) + ")",
    };
  }
  if (!response || response.ok !== true) {
    const detail = response && response.error ? response.error : "unknown error";
    return { ok: false, message: "Grimoire error: " + detail };
  }
  if (response.status === "skipped") {
    return { ok: true, message: "Already in your library (skipped)." };
  }
  const keys = Array.isArray(response.keys) ? response.keys : [];
  return {
    ok: true,
    message: keys.length ? "Added: " + keys.join(", ") : "Added to Grimoire.",
  };
}

function notify(title, message) {
  try {
    api.notifications.create({
      type: "basic",
      iconUrl: api.runtime.getURL("icons/icon-128.png"),
      title,
      message,
    });
  } catch {
    // Notifications are best-effort; ignore if unavailable.
  }
}

async function addActiveTab() {
  const tabs = await api.tabs.query({ active: true, currentWindow: true });
  const tab = tabs && tabs[0];
  if (!tab || !tab.url) {
    notify("Grimoire", "No active tab URL to import.");
    return;
  }
  const result = await sendToGrimoire(resolveInput(tab.url));
  notify(result.ok ? "Grimoire" : "Grimoire — failed", result.message);
}

// Toolbar keyboard shortcut.
api.commands.onCommand.addListener((command) => {
  if (command === "add-current-tab") addActiveTab();
});

// Right-click → "Add to Grimoire" on the page or a link.
api.runtime.onInstalled.addListener(() => {
  api.contextMenus.create({
    id: "grimoire-add-page",
    title: "Add page to Grimoire",
    contexts: ["page"],
  });
  api.contextMenus.create({
    id: "grimoire-add-link",
    title: "Add link to Grimoire",
    contexts: ["link"],
  });
});

api.contextMenus.onClicked.addListener(async (info, tab) => {
  const target =
    info.menuItemId === "grimoire-add-link" ? info.linkUrl : (tab && tab.url);
  if (!target) return;
  const result = await sendToGrimoire(resolveInput(target));
  notify(result.ok ? "Grimoire" : "Grimoire — failed", result.message);
});

// Messages from the popup.
api.runtime.onMessage.addListener((message, _sender, sendResponse) => {
  if (message && message.type === "add") {
    sendToGrimoire(resolveInput(message.url), message.force === true).then(
      sendResponse
    );
    return true; // keep the channel open for the async reply
  }
  return false;
});
