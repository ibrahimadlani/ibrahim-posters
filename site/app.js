// Playground for the poster service.
//
// Vanilla, no build step: the page has to be servable by GitHub Pages, by
// `python3 -m http.server`, or by opening the file — anything requiring a
// toolchain would be a worse demo of a service whose point is that it is easy
// to call.

const TMDB_IMAGE = "https://image.tmdb.org/t/p";

const el = (id) => document.getElementById(id);
const state = {
  kind: "movie",
  catalogue: null,
  poster: "auto",
  logo: "auto",
  badges: [{ text: "#13 IMDb", style: "accent" }],
};

const baseUrl = () => el("base-url").value.replace(/\/+$/, "");

/* ---------------------------------------------------------------- requests */

/// Issues a request and records it in the HTTP log.
///
/// Everything goes through here so the log is complete by construction rather
/// than by remembering to call it — a log that shows some requests is worse
/// than none, because it invites the wrong conclusion about the ones missing.
async function call(method, path, body) {
  const started = performance.now();
  let response, text;

  try {
    response = await fetch(baseUrl() + path, {
      method,
      headers: body ? { "content-type": "application/json" } : {},
      body: body ? JSON.stringify(body) : undefined,
    });
  } catch (cause) {
    logEntry(method, path, 0, performance.now() - started,
      `Could not reach ${baseUrl()}.\n\n` +
      `The service is not running, or it is running without CORS enabled.\n` +
      `CORS arrived alongside this page, so a service older than it will\n` +
      `refuse the request before it reaches any endpoint.`);
    throw new ServiceError(null, `Cannot reach ${baseUrl()}`,
      "Start the service, or point Base URL at a running one. " +
      "A browser cannot show why a cross-origin request failed, so this " +
      "covers both 'not running' and 'running without CORS'.");
  }

  const isImage = (response.headers.get("content-type") || "").startsWith("image/");
  const elapsed = performance.now() - started;

  if (isImage) {
    const blob = await response.blob();
    logEntry(method, path, response.status, elapsed,
      `${blob.type}, ${blob.size.toLocaleString()} bytes`);
    return { response, blob, elapsed };
  }

  text = await response.text();
  logEntry(method, path, response.status, elapsed, text || "(empty body)");

  if (!response.ok) {
    // Every failure is an RFC 9457 document, so one branch handles all of
    // them — but a body that is not one still has to surface readably.
    let problem = null;
    try { problem = JSON.parse(text); } catch { /* not a problem document */ }
    throw new ServiceError(problem, problem?.detail || `HTTP ${response.status}`,
      problem?.hint, problem?.type);
  }

  return { response, json: text ? JSON.parse(text) : null, elapsed };
}

class ServiceError extends Error {
  constructor(problem, detail, hint, docs) {
    super(detail);
    this.problem = problem;
    this.hint = hint;
    this.docs = docs;
  }
}

/* -------------------------------------------------------------------- log */

function logEntry(method, path, status, ms, body) {
  const entry = document.createElement("div");
  entry.className = "log-entry";
  const klass = status === 0 ? "c5" : `c${String(status)[0]}`;
  entry.innerHTML = `
    <div class="log-head">
      <span class="verb">${method}</span>
      <span class="path"></span>
      <span class="code ${klass}">${status || "ERR"}</span>
      <span style="color:var(--dim)">${Math.round(ms)} ms</span>
    </div>
    <div class="log-body"><pre></pre></div>`;
  entry.querySelector(".path").textContent = path;
  entry.querySelector("pre").textContent = pretty(body);
  entry.querySelector(".log-head").onclick = () => entry.classList.toggle("open");
  el("log").prepend(entry);
  while (el("log").children.length > 20) el("log").lastChild.remove();
}

const pretty = (text) => {
  try { return JSON.stringify(JSON.parse(text), null, 2); } catch { return text; }
};

/* ---------------------------------------------------------------- artwork */

async function browse() {
  const id = el("tmdb-id").value.trim();
  if (!id) return;

  el("browse").disabled = true;
  el("browse").innerHTML = '<span class="spinner"></span>';
  clearError();

  try {
    const language = el("language").value.trim() || "en";
    const { json } = await call("GET",
      `/v1/artwork/${state.kind}/${id}?language=${encodeURIComponent(language)}`);

    state.catalogue = json;
    state.poster = "auto";
    state.logo = "auto";
    renderThumbs();
    el("artwork-section").hidden = false;

    // A title with no textless poster falls back to everything it has. Say so,
    // rather than leaving a picker full of titled posters looking like a bug.
    const anyTextless = json.posters.some(
      (option) => option.language === null || option.language === "xx");
    el("poster-note").textContent = anyTextless
      ? "Textless artwork only — the version without a title treatment, so the logo does not duplicate one already printed."
      : "This title has no textless poster, so every one it has is offered instead. Expect the title to show through behind the logo.";
    el("poster-note").style.color = anyTextless ? "" : "var(--warn)";
  } catch (error) {
    showError(error);
    el("artwork-section").hidden = true;
  } finally {
    el("browse").disabled = false;
    el("browse").textContent = "Browse artwork";
    updatePreview();
  }
}

function renderThumbs() {
  const { posters, logos } = state.catalogue;
  el("poster-count").textContent = `(${posters.length})`;
  el("logo-count").textContent = `(${logos.length})`;

  el("poster-thumbs").replaceChildren(
    ...posters.slice(0, 24).map((option, index) =>
      thumb(option, index === 0, () => { state.poster = option.path; renderThumbs(); updatePreview(); },
        state.poster === "auto" ? index === 0 : state.poster === option.path)));

  const none = document.createElement("button");
  none.type = "button";
  none.className = "thumb";
  none.setAttribute("aria-pressed", state.logo === "none");
  none.innerHTML = `<span style="color:var(--dim);font-size:12px">no logo</span>`;
  none.onclick = () => { state.logo = "none"; renderThumbs(); updatePreview(); };

  el("logo-thumbs").replaceChildren(
    none,
    ...logos.slice(0, 12).map((option, index) =>
      thumb(option, index === 0, () => { state.logo = option.path; renderThumbs(); updatePreview(); },
        state.logo === "auto" ? index === 0 : state.logo === option.path)));
}

function thumb(option, isDefault, onPick, selected) {
  const button = document.createElement("button");
  button.type = "button";
  button.className = "thumb";
  button.setAttribute("aria-pressed", String(selected));
  button.title = `${option.path}\n${option.language ?? "no language"} · ${option.vote_average} · ${option.width}×${option.height}`;

  const img = document.createElement("img");
  img.loading = "lazy";
  img.src = `${TMDB_IMAGE}/w185${option.path}`;
  img.alt = "";
  button.append(img);

  // The label carries whichever fact is not already obvious. For posters the
  // service filters to textless, so "no lang" on every one of them would be
  // noise -- what a picker actually needs to know is which one is the default
  // and, when the fallback ran, that these carry a title after all.
  const textless = option.language === null || option.language === "xx";
  const label = isDefault ? "default" : (textless ? "" : option.language);
  if (label) {
    const tag = document.createElement("span");
    tag.className = isDefault ? "tag none" : "tag";
    tag.textContent = label;
    button.append(tag);
  }

  button.onclick = onPick;
  return button;
}

/* --------------------------------------------------------------- request */

function buildRequest() {
  const request = {
    [state.kind === "movie" ? "tmdb_movie_id" : "tmdb_tv_id"]:
      Number(el("tmdb-id").value) || 0,
  };

  const language = el("language").value.trim();
  if (language && language !== "en") request.language = language;
  if (el("preset").value !== "standard") request.preset = el("preset").value;
  if (el("width").value !== "w1000") request.width = el("width").value;
  if (state.poster !== "auto") request.poster = state.poster;
  if (state.logo !== "auto") request.logo = state.logo;

  const badges = state.badges.filter((badge) => badge.text.trim());
  if (badges.length) request.badges = badges;

  if (el("use-overrides").checked) {
    request.overrides = {
      blur_band_fraction: Number(el("blur-band").value),
      blur_sigma: Number(el("blur-sigma").value),
      darken_strength: Number(el("darken").value),
      logo_width_fraction: Number(el("logo-width").value),
    };
  }
  return request;
}

function updatePreview() {
  const json = JSON.stringify(buildRequest(), null, 2)
    .replace(/&/g, "&amp;").replace(/</g, "&lt;")
    .replace(/"([^"]+)":/g, '<span class="k">"$1"</span>:')
    .replace(/: "([^"]*)"/g, ': <span class="s">"$1"</span>')
    .replace(/: (-?\d+\.?\d*)/g, ': <span class="n">$1</span>');
  el("request-preview").innerHTML =
    `<span style="color:var(--dim)">POST ${baseUrl()}/v1/posters</span>\n\n${json}`;
}

/* ---------------------------------------------------------------- render */

async function render() {
  el("render").disabled = true;
  el("render").innerHTML = '<span class="spinner"></span>';
  clearError();

  try {
    const { json: created } = await call("POST", "/v1/posters", buildRequest());
    const path = `/v1/posters/${created.key}.webp`;
    const { response, blob, elapsed } = await call("GET", path);

    el("frame").replaceChildren(Object.assign(new Image(), {
      src: URL.createObjectURL(blob),
      alt: "Rendered poster",
    }));

    const cache = response.headers.get("x-cache") || "?";
    el("meta").hidden = false;
    el("meta").innerHTML = `
      <span class="pill ${cache === "HIT" ? "hit" : "miss"}">x-cache: ${cache}</span>
      <span class="pill">${Math.round(elapsed)} ms</span>
      <span class="pill">${(blob.size / 1024).toFixed(0)} KB</span>
      <span>key <b>${created.key.slice(0, 12)}…</b></span>
      <span>request <b>${response.headers.get("x-request-id") || "?"}</b></span>`;
  } catch (error) {
    showError(error);
  } finally {
    el("render").disabled = false;
    el("render").textContent = "Create poster";
  }
}

/* ----------------------------------------------------------------- errors */

function showError(error) {
  const slot = el("error-slot");
  const box = document.createElement("div");
  box.className = "error";

  const code = error.problem?.code || "unreachable";
  const heading = document.createElement("h3");
  heading.textContent = code;
  box.append(heading);

  const detail = document.createElement("p");
  detail.textContent = error.message;
  box.append(detail);

  if (error.hint) {
    const hint = document.createElement("p");
    hint.className = "hint";
    hint.textContent = error.hint;
    box.append(hint);
  }
  if (error.docs) {
    const link = document.createElement("a");
    link.href = error.docs;
    link.target = "_blank";
    link.rel = "noreferrer";
    link.textContent = "What this code means →";
    box.append(link);
  }
  slot.replaceChildren(box);
}

const clearError = () => el("error-slot").replaceChildren();

/* ---------------------------------------------------------------- badges */

function renderBadges() {
  el("badges").replaceChildren(...state.badges.map((badge, index) => {
    const row = document.createElement("div");
    row.className = "badge-row";

    const text = document.createElement("input");
    text.type = "text";
    text.value = badge.text;
    text.placeholder = "Badge text";
    text.oninput = () => { badge.text = text.value; updatePreview(); };

    const style = document.createElement("select");
    for (const name of ["solid", "outline", "accent"]) {
      style.append(new Option(name, name, false, badge.style === name));
    }
    style.onchange = () => { badge.style = style.value; updatePreview(); };

    const remove = document.createElement("button");
    remove.type = "button";
    remove.textContent = "×";
    remove.onclick = () => { state.badges.splice(index, 1); renderBadges(); updatePreview(); };

    row.append(text, style, remove);
    return row;
  }));
  el("add-badge").disabled = state.badges.length >= 6;
}

/* ------------------------------------------------------------------ setup */

async function checkService() {
  try {
    const response = await fetch(baseUrl() + "/healthz");
    const up = response.ok;
    el("status-dot").className = `dot ${up ? "up" : "down"}`;
    el("status-text").textContent = up ? baseUrl() : `HTTP ${response.status}`;
    if (up) loadPresets();
  } catch {
    el("status-dot").className = "dot down";
    el("status-text").textContent = "unreachable";
  }
}

async function loadPresets() {
  try {
    const response = await fetch(baseUrl() + "/v1/presets");
    const { presets } = await response.json();
    el("preset").replaceChildren(
      ...presets.map((preset) => new Option(preset.name, preset.name)));
  } catch { /* the default option stays */ }
}

for (const button of document.querySelectorAll("#kind-seg button")) {
  button.onclick = () => {
    state.kind = button.dataset.kind;
    for (const other of document.querySelectorAll("#kind-seg button")) {
      other.setAttribute("aria-pressed", String(other === button));
    }
    el("tmdb-id").value = state.kind === "movie" ? "27205" : "1396";
    el("artwork-section").hidden = true;
    state.catalogue = null;
    updatePreview();
  };
}

for (const tab of document.querySelectorAll(".tabs button")) {
  tab.onclick = () => {
    for (const other of document.querySelectorAll(".tabs button")) {
      other.setAttribute("aria-pressed", String(other === tab));
    }
    for (const panel of document.querySelectorAll("[data-panel]")) {
      panel.hidden = panel.dataset.panel !== tab.dataset.tab;
    }
  };
}

el("browse").onclick = browse;
el("render").onclick = render;
el("add-badge").onclick = () => {
  state.badges.push({ text: "", style: "solid" });
  renderBadges();
};
el("use-overrides").onchange = (event) => {
  el("overrides").hidden = !event.target.checked;
  updatePreview();
};
for (const id of ["base-url", "tmdb-id", "language", "preset", "width",
                  "blur-band", "blur-sigma", "darken", "logo-width"]) {
  el(id).addEventListener("input", updatePreview);
}
el("base-url").addEventListener("change", checkService);

renderBadges();
updatePreview();
checkService();
