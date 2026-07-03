// wifi-radar — frontend SPA logic.
//
// Wires the SSE stream into the in-browser store, drives the radar render
// loop, and posts tag edits back to the API.

(function () {
    "use strict";

    const store = {
        devices: new Map(), // mac -> {mac, rssi_dbm, channel, last_kind, last_seen_unix}
        tags: {},            // mac -> {label, icon, color}
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

        function loop() {
            radar.draw(Array.from(store.devices.values()), store.tags);
            requestAnimationFrame(loop);
        }
        requestAnimationFrame(loop);
    });
})();