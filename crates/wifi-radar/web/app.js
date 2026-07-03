// wifi-radar — frontend SPA logic.
//
// Wires the SSE stream into the in-browser store, drives the radar render
// loop, and posts tag edits back to the API.

(function () {
    "use strict";

    const store = {
        devices: new Map(), // mac -> {mac, rssi_dbm, channel, last_kind, last_seen_unix}
        tags: {},            // mac -> {label, icon, color}
        vitals: null,        // latest /api/vitals reading (CSI human sensing)
    };

    let radar = null;
    let eventSource = null;
    let selectedMac = null;

    function $(id) { return document.getElementById(id); }

    function setStatus(text, klass) {
        const el = $("status");
        el.textContent = text;
        el.classList.remove("connected", "disconnected");
        if (klass) el.classList.add(klass);
    }

    function applyEvent(ev) {
        store.devices.set(ev.mac, ev);
    }

    function renderDeviceList() {
        const ul = $("device-list");
        const macs = Array.from(store.devices.keys()).sort();
        ul.innerHTML = "";
        for (const mac of macs) {
            const d = store.devices.get(mac);
            const tag = store.tags[mac];
            const li = document.createElement("li");
            if (mac === selectedMac) li.classList.add("selected");
            li.dataset.mac = mac;

            const left = document.createElement("div");
            const label = document.createElement("div");
            label.className = "label" + (tag ? "" : " unset");
            label.textContent = tag ? tag.label : "(unlabeled)";
            const macEl = document.createElement("div");
            macEl.className = "mac";
            macEl.textContent = mac;
            left.appendChild(label);
            left.appendChild(macEl);

            const right = document.createElement("div");
            right.className = "rssi";
            right.textContent = `${d.rssi_dbm} dBm · ch${d.channel}`;

            li.appendChild(left);
            li.appendChild(right);
            li.addEventListener("click", () => selectMac(mac));
            ul.appendChild(li);
        }
    }

    function selectMac(mac) {
        selectedMac = mac;
        const tag = store.tags[mac] || {};
        $("tag-mac").value = mac;
        $("tag-label").value = tag.label || "";
        $("tag-icon").value = tag.icon || "generic";
        $("tag-color").value = tag.color || "#7fdcff";
        renderDeviceList();
    }

    async function refreshSnapshot() {
        try {
            const [devResp, tagResp] = await Promise.all([
                fetch("/api/devices").then((r) => r.json()),
                fetch("/api/tags").then((r) => r.json()),
            ]);
            store.devices.clear();
            for (const d of devResp.devices) store.devices.set(d.mac, d);
            store.tags = (tagResp && tagResp.tags) || {};
            renderDeviceList();
        } catch (e) {
            console.warn("snapshot refresh failed", e);
        }
    }

    function connect() {
        eventSource = new EventSource("/api/events");
        eventSource.onopen = () => setStatus("live", "connected");
        eventSource.onerror = () => setStatus("disconnected", "disconnected");
        eventSource.onmessage = (msg) => {
            try {
                const ev = JSON.parse(msg.data);
                applyEvent(ev);
            } catch (_) {
                // ignore malformed frames
            }
        };
    }

    // CSI vitals: poll /api/vitals ~1/s and paint the human-sensing panel.
    // Breathing needs a ~20 s window, so a 1 s poll is plenty.
    function pct(x) { return `${Math.round((x || 0) * 100)}%`; }

    async function refreshVitals() {
        let v;
        try {
            v = await fetch("/api/vitals").then((r) => r.json());
        } catch (_) {
            return; // transient; keep last paint
        }
        store.vitals = v; // drive the radar contact
        const panel = $("vitals");
        panel.dataset.present = String(!!v.presence);

        const dot = $("vitals-dot");
        dot.className = "vitals-dot" + (v.presence ? " on" : "");
        $("vitals-presence-text").textContent = v.presence
            ? `present · ${v.motion_level} motion`
            : (v.frames_in_window > 0 ? "no one detected" : "no signal");

        const breath = v.breathing_rate_bpm > 0 ? v.breathing_rate_bpm.toFixed(1) : "–";
        const heart = v.heart_rate_bpm > 0 ? Math.round(v.heart_rate_bpm) : "–";
        $("vitals-breath").textContent = breath;
        $("vitals-heart").textContent = heart;
        $("vitals-breath-conf").textContent =
            v.breathing_rate_bpm > 0 ? `conf ${pct(v.breathing_confidence)}` : "";
        $("vitals-heart-conf").textContent =
            v.heart_rate_bpm > 0 ? `conf ${pct(v.heartbeat_confidence)}` : "";

        $("vitals-meta").textContent = v.sample_rate_hz > 0
            ? `${v.subcarrier_count} subcarriers · ${v.sample_rate_hz.toFixed(0)} Hz · ${v.frames_in_window} frames`
            : "waiting for nexmon CSI on :5500…";
    }

    async function saveTag() {
        const mac = $("tag-mac").value.trim();
        const label = $("tag-label").value.trim();
        const icon = $("tag-icon").value;
        const color = $("tag-color").value;
        if (!mac) return;
        const resp = await fetch("/api/tags", {
            method: "POST",
            headers: { "content-type": "application/json" },
            body: JSON.stringify({ mac, label, icon, color }),
        });
        if (resp.ok) {
            store.tags[mac] = { label, icon, color };
            renderDeviceList();
        }
    }

    async function deleteTag() {
        const mac = $("tag-mac").value.trim();
        if (!mac) return;
        const resp = await fetch(`/api/tags/${encodeURIComponent(mac)}`, {
            method: "DELETE",
        });
        if (resp.ok) {
            delete store.tags[mac];
            renderDeviceList();
        }
    }

    window.addEventListener("DOMContentLoaded", () => {
        radar = new Radar($("radar"));
        $("tag-save").addEventListener("click", saveTag);
        $("tag-delete").addEventListener("click", deleteTag);
        connect();
        refreshSnapshot();
        setInterval(refreshSnapshot, 10_000);
        refreshVitals();
        setInterval(refreshVitals, 1_000);

        function loop() {
            radar.draw(Array.from(store.devices.values()), store.tags, store.vitals);
            requestAnimationFrame(loop);
        }
        requestAnimationFrame(loop);
    });
})();