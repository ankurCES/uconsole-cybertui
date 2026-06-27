// cyberdeck-web: vanilla JS front-end. Connects to /api/ws for live updates
// and to /api/<resource> for actions. No framework, no build step.

const NAV = [
  { id: "system",   label: "System" },
  { id: "network",  label: "Network" },
  { id: "bluetooth",label: "Bluetooth" },
  { id: "power",    label: "Power" },
  { id: "display",  label: "Display" },
  { id: "audio",    label: "Audio" },
  { id: "storage",  label: "Storage" },
  { id: "services", label: "Services" },
  { id: "packages", label: "Packages" },
  { id: "processes",label: "Processes" },
  { id: "logs",     label: "Logs" },
  { id: "settings", label: "Settings" },
];

const state = { view: "system", live: {}, upSince: Date.now() };

const $ = (id) => document.getElementById(id);
const esc = (s) => String(s).replace(/[<>&]/g, c => ({"<":"&lt;",">":"&gt;","&":"&amp;"}[c]));

function renderNav() {
  const el = $("nav");
  el.innerHTML = NAV.map(n =>
    `<div class="item${n.id===state.view?" active":""}" data-id="${n.id}">${n.label}</div>`
  ).join("");
  el.querySelectorAll(".item").forEach(node => {
    node.addEventListener("click", () => { state.view = node.dataset.id; renderNav(); renderView(); });
  });
}

function fmtUp(s) {
  const d = Math.floor(s/86400), h = Math.floor((s%86400)/3600), m = Math.floor((s%3600)/60);
  return d > 0 ? `${d}d ${h}h` : h > 0 ? `${h}h ${m}m` : `${m}m`;
}
function fmtBytes(kb) {
  const mb = kb/1024; if (mb < 1024) return `${mb.toFixed(0)}M`;
  return `${(mb/1024).toFixed(1)}G`;
}
function bar(pct, w=20) {
  const f = Math.round(pct/100*w), e = w-f;
  return "█".repeat(f) + "░".repeat(e);
}

async function api(path, opts={}) {
  const tok = token();
  const headers = Object.assign({ "Content-Type": "application/json" }, opts.headers || {});
  if (tok) headers["Authorization"] = `Bearer ${tok}`;
  const r = await fetch(path, Object.assign({}, opts, { headers }));
  if (!r.ok) {
    let msg; try { msg = (await r.json()).error; } catch { msg = r.statusText; }
    throw new Error(`${r.status} ${msg}`);
  }
  if (r.status === 204) return null;
  return r.json();
}

function token() {
  // Look at the URL once, then at localStorage.
  let t = localStorage.getItem("cdk_token");
  if (!t) {
    const url = new URL(location.href);
    t = url.searchParams.get("token");
    if (t) localStorage.setItem("cdk_token", t);
  }
  return t;
}

function renderHeader() {
  const s = state.live;
  $("hdr-host").textContent   = s.hostname || "…";
  $("hdr-kernel").textContent = s.kernel   || "…";
  $("hdr-up").textContent     = s.uptime_secs != null ? fmtUp(s.uptime_secs) : "…";
  $("hdr-load").textContent   = s.loadavg ? s.loadavg.map(x => x.toFixed(2)).join(" ") : "…";
  $("hdr-mem").textContent    = s.mem_total_kb ? `${fmtBytes(s.mem_total_kb - s.mem_avail_kb)} / ${fmtBytes(s.mem_total_kb)}` : "…";
  $("hdr-ssid").hidden = !s.active_ssid;
  $("hdr-ssid-v").textContent = s.active_ssid || "";
  const b = s.battery;
  $("hdr-bat").hidden = !b;
  $("hdr-bat-v").textContent = b ? `${b.capacity}%` : "";
  const t = (s.thermals||[]).find(x => x.label.toLowerCase().includes("cpu")) || (s.thermals||[])[0];
  $("hdr-temp").hidden = !t;
  $("hdr-temp-v").textContent = t ? `${t.temp_c.toFixed(0)}°C` : "";
}

function renderView() {
  const v = $("view");
  const fn = views[state.view];
  v.innerHTML = fn ? fn() : "<h1>unknown view</h1>";
}

const views = {
  system() {
    const s = state.live;
    return `<h1>System</h1>
      <div class="row"><div class="k">hostname</div><div class="v">${esc(s.hostname||"")}</div></div>
      <div class="row"><div class="k">kernel</div><div class="v">${esc(s.kernel||"")}</div></div>
      <div class="row"><div class="k">uptime</div><div class="v">${esc(fmtUp(s.uptime_secs||0))}</div></div>
      <div class="row"><div class="k">load</div><div class="v">${esc((s.loadavg||[]).map(x=>x.toFixed(2)).join(" "))}</div></div>
      <div class="row"><div class="k">memory</div><div class="v">${esc(fmtBytes((s.mem_total_kb||0)-(s.mem_avail_kb||0)))} / ${esc(fmtBytes(s.mem_total_kb||0))}</div></div>
      <h2>Thermal</h2>
      ${(s.thermals||[]).map(t => `<div class="row"><div class="k">${esc(t.label)}</div><div class="v ${t.temp_c>75?"err":t.temp_c>60?"warn":"ok"}">${t.temp_c.toFixed(1)}°C</div></div>`).join("")}`;
  },
  network() {
    const ifs = state.live.interfaces || [];
    return `<h1>Network</h1>
      <h2>Interfaces</h2>
      <table><thead><tr><th>name</th><th>state</th><th>ipv4</th></tr></thead><tbody>
      ${ifs.map(i => `<tr><td>${esc(i.name)}</td><td class="${i.state.toLowerCase()==="up"?"ok":"warn"}">${esc(i.state)}</td><td>${esc((i.ipv4||[]).join(", "))}</td></tr>`).join("")}
      </tbody></table>`;
  },
  power() {
    const b = state.live.battery;
    return `<h1>Power</h1>
      ${b ? `<div class="row"><div class="k">battery</div><div class="v ${b.capacity<20?"err":b.capacity<50?"warn":"ok"}">${bar(b.capacity)} ${b.capacity}% · ${esc(b.status)}</div></div>` : `<div class="row"><div class="k">power</div><div class="v">AC only</div></div>`}
      <h2>Actions</h2>
      <button onclick="post('/api/power/suspend')">suspend</button>
      <button onclick="post('/api/power/hibernate')">hibernate</button>
      <button onclick="confirmThen('/api/power/reboot','Reboot?')">reboot</button>
      <button onclick="confirmThen('/api/power/shutdown','Shut down?')">poweroff</button>`;
  },
  display() {
    return `<h1>Display</h1>
      <h2>Brightness</h2>
      <div class="row"><div class="k">value</div><div class="v" id="bright">…</div></div>
      <button onclick="adjBright(-5)">−5</button>
      <button onclick="adjBright(+5)">+5</button>
      <div id="disp-list"></div>`;
  },
  audio() {
    return `<h1>Audio</h1><div id="sinks"></div>`;
  },
  storage() {
    return `<h1>Storage</h1><div id="fs"></div>`;
  },
  services() {
    return `<h1>Services</h1><div id="svcs"></div>`;
  },
  packages() {
    return `<h1>Packages</h1>
      <button onclick="post('/api/packages/update')">apt update</button>
      <button onclick="post('/api/packages/upgrade')">apt upgrade</button>
      <div id="upg"></div>`;
  },
  processes() {
    return `<h1>Processes</h1><div id="procs"></div>`;
  },
  logs() {
    return `<h1>Logs</h1>
      <button onclick="fetchLogs()">fetch last 50</button>
      <div class="scrollable" id="logs"></div>`;
  },
  bluetooth() { return `<h1>Bluetooth</h1><div id="bt"></div>`; },
  settings() {
    return `<h1>Settings</h1>
      <div class="row"><div class="k">auth token</div><div class="v accent">${esc(token()||"(off)")}</div></div>`;
  },
};

async function post(path) {
  try { await api(path, { method: "POST" }); } catch (e) { alert(e.message); }
}
async function confirmThen(path, msg) {
  if (!confirm(msg)) return;
  try { await api(path, { method: "POST" }); } catch (e) { alert(e.message); }
}
async function adjBright(d) {
  const cur = parseInt(($("bright")?.textContent || "0").match(/\d+/)?.[0] || "0", 10);
  const next = Math.max(0, Math.min(100, cur + d));
  try { await api("/api/display/brightness", { method: "POST", body: JSON.stringify({ value: next }) });
        $("bright").textContent = `${next}%`; } catch (e) { alert(e.message); }
}

async function refreshView() {
  const v = state.view;
  try {
    if (v === "display") {
      const b = await api("/api/display/brightness");
      $("bright").textContent = `${b}%`;
      const ds = await api("/api/display/outputs");
      $("disp-list").innerHTML = "<h2>Outputs</h2>" + ds.map(d =>
        `<div class="row"><div class="k">${esc(d.name)}</div><div class="v ${d.enabled?"ok":"warn"}">${d.enabled?"on":"off"}</div><div class="v">${esc(d.mode)}</div></div>`
      ).join("");
    } else if (v === "audio") {
      const s = await api("/api/audio/sinks");
      $("sinks").innerHTML = "<h2>Sinks</h2>" + s.map(k =>
        `<div class="row"><div class="k">${esc(k.name)}</div><div class="v ${k.muted?"warn":"ok"}">${k.muted?"muted":`${k.volume}%`}</div></div>`).join("");
    } else if (v === "storage") {
      const f = await api("/api/storage/df");
      $("fs").innerHTML = "<table><thead><tr><th>source</th><th>fstype</th><th>size</th><th>used</th><th>avail</th><th>use%</th><th>mount</th></tr></thead><tbody>" +
        f.map(m => `<tr><td>${esc(m.source)}</td><td>${esc(m.fstype)}</td><td>${esc(m.size)}</td><td>${esc(m.used)}</td><td>${esc(m.avail)}</td><td class="${m.use_pct>90?"err":m.use_pct>75?"warn":"ok"}">${m.use_pct}%</td><td>${esc(m.mounted_on)}</td></tr>`).join("") + "</tbody></table>";
    } else if (v === "services") {
      const s = await api("/api/services");
      $("svcs").innerHTML = "<table><thead><tr><th>unit</th><th>active</th><th>sub</th><th>description</th><th></th></tr></thead><tbody>" +
        s.map(x => `<tr><td>${esc(x.unit)}</td><td class="${x.active==="active"?"ok":x.active==="failed"?"err":"warn"}">${esc(x.active)}</td><td>${esc(x.sub)}</td><td>${esc(x.description)}</td>` +
        `<td>${["start","stop","restart","enable","disable"].map(o => `<button onclick="post('/api/services/${encodeURIComponent(x.unit)}/${o}')">${o}</button>`).join(" ")}</td></tr>`).join("") + "</tbody></table>";
    } else if (v === "packages") {
      const u = await api("/api/packages/upgradable");
      $("upg").innerHTML = "<h2>Upgradable</h2><ul>" + u.map(p => `<li>${esc(p.name)} <button onclick="post('/api/packages/install', '${esc(p.name)}')">install</button></li>`).join("") + "</ul>";
    } else if (v === "processes") {
      const p = await api("/api/processes");
      $("procs").innerHTML = "<table><thead><tr><th>PID</th><th>user</th><th>CPU%</th><th>MEM%</th><th>cmd</th></tr></thead><tbody>" +
        p.slice(0, 80).map(x => `<tr><td>${x.pid}</td><td>${esc(x.user)}</td><td>${x.cpu.toFixed(1)}</td><td>${x.mem.toFixed(1)}</td><td>${esc(x.command)}</td></tr>`).join("") + "</tbody></table>";
    } else if (v === "bluetooth") {
      const d = await api("/api/bluetooth/devices");
      $("bt").innerHTML = "<table><thead><tr><th>mac</th><th>name</th><th>paired</th><th>connected</th><th></th></tr></thead><tbody>" +
        d.map(x => `<tr><td>${esc(x.mac)}</td><td>${esc(x.name)}</td><td>${x.paired?"✓":""}</td><td>${x.connected?"✓":""}</td>` +
        `<td><button onclick="post('/api/bluetooth/pair', '${esc(x.mac)}')">pair</button> <button onclick="post('/api/bluetooth/connect', '${esc(x.mac)}')">connect</button></td></tr>`).join("") + "</tbody></table>";
    }
  } catch (e) { console.warn(e); }
}

async function fetchLogs() {
  // The TUI has a live log tail; the web has a "fetch last 50" button that
  // shells out to journalctl via the host. We surface a placeholder here —
  // the standalone binary could expose a /api/logs/recent endpoint later.
  $("logs").innerHTML = "<div>(logs endpoint not yet wired on the web)</div>";
}

function startWS() {
  const proto = location.protocol === "https:" ? "wss:" : "ws:";
  const url = `${proto}//${location.host}/api/ws?token=${encodeURIComponent(token()||"")}`;
  const ws = new WebSocket(url);
  ws.onmessage = (ev) => {
    try {
      state.live = JSON.parse(ev.data);
      renderHeader();
      renderView();
      refreshView();
    } catch (e) { console.warn(e); }
  };
  ws.onclose = () => setTimeout(startWS, 2000);
  ws.onerror = () => ws.close();
}

function tickClock() {
  const d = new Date();
  $("clock").textContent = d.toTimeString().slice(0,8);
  setTimeout(tickClock, 1000 - d.getMilliseconds());
}

document.addEventListener("keydown", (e) => {
  const idx = "1234567890".indexOf(e.key);
  if (idx >= 0 && idx < NAV.length) { state.view = NAV[idx].id; renderNav(); renderView(); refreshView(); }
  if (e.key === "r") refreshView();
});

renderNav();
renderView();
refreshView();
tickClock();
startWS();
