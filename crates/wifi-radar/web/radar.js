// OpenScope-style radar canvas: sweep line, fading trail, device dots.
//
// Exposes a single global `Radar` object with `draw(devices, tags)` and a
// status `ping()` for the connection indicator.

(function () {
    "use strict";

    const ACCENT = "#7fdcff";
    const DIM = "rgba(127, 220, 255, 0.15)";
    const GRID = "rgba(127, 220, 255, 0.08)";
    const BG = "#0b0f14";

    // Trail buffer: previous-frame pixels fade by TRAIL_DECAY each frame.
    const TRAIL_DECAY = 0.94;

    function Radar(canvas) {
        this.canvas = canvas;
        this.ctx = canvas.getContext("2d");
        this.trail = null; // OffscreenCanvas for fading trail
        this.angle = 0;    // Sweep angle in radians.
        this.lastT = performance.now();
        this.size = { w: 0, h: 0 };
        this.resize();
    }

    Radar.prototype.resize = function () {
        const rect = this.canvas.getBoundingClientRect();
        const dpr = window.devicePixelRatio || 1;
        this.canvas.width = Math.max(64, Math.floor(rect.width * dpr));
        this.canvas.height = Math.max(64, Math.floor(rect.height * dpr));
        this.size.w = this.canvas.width;
        this.size.h = this.canvas.height;
        this.trail = document.createElement("canvas");
        this.trail.width = this.canvas.width;
        this.trail.height = this.canvas.height;
    };

    Radar.prototype.draw = function (devices, tags) {
        const now = performance.now();
        const dt = Math.min(0.1, (now - this.lastT) / 1000);
        this.lastT = now;
        this.angle = (this.angle + dt * 1.6) % (Math.PI * 2);

        // Init / resize trail if needed.
        if (
            !this.trail ||
            this.trail.width !== this.canvas.width ||
            this.trail.height !== this.canvas.height
        ) {
            this.resize();
        }

        const ctx = this.ctx;
        const tctx = this.trail.getContext("2d");
        const W = this.size.w;
        const H = this.size.h;
        const cx = W / 2;
        const cy = H / 2;
        const r = Math.min(cx, cy) - 6 * (window.devicePixelRatio || 1);

        // Fade the trail buffer.
        tctx.fillStyle = `rgba(11, 15, 20, ${1 - TRAIL_DECAY})`;
        tctx.fillRect(0, 0, W, H);

        // Paint trail on top of the base canvas.
        ctx.fillStyle = BG;
        ctx.fillRect(0, 0, W, H);
        ctx.drawImage(this.trail, 0, 0);

        // Grid + range rings.
        ctx.strokeStyle = GRID;
        ctx.lineWidth = 1;
        for (let i = 1; i <= 4; i++) {
            ctx.beginPath();
            ctx.arc(cx, cy, (r * i) / 4, 0, Math.PI * 2);
            ctx.stroke();
        }
        ctx.beginPath();
        ctx.moveTo(cx - r, cy);
        ctx.lineTo(cx + r, cy);
        ctx.moveTo(cx, cy - r);
        ctx.lineTo(cx, cy + r);
        ctx.stroke();

        // Sweep wedge.
        const sweepLen = Math.PI * 0.18;
        const grad = ctx.createConicGradient
            ? ctx.createConicGradient(this.angle - sweepLen, cx, cy)
            : null;
        if (grad) {
            grad.addColorStop(0, "rgba(127, 220, 255, 0.0)");
            grad.addColorStop(1, "rgba(127, 220, 255, 0.35)");
            ctx.fillStyle = grad;
            ctx.beginPath();
            ctx.moveTo(cx, cy);
            ctx.arc(cx, cy, r, this.angle - sweepLen, this.angle);
            ctx.closePath();
            ctx.fill();
        }

        // Devices.
        for (let i = 0; i < devices.length; i++) {
            const d = devices[i];
            const tag = tags[d.mac];
            // RSSI (dBm, -90..-20) → radius (0..r). Closer = bigger.
            const norm = Math.max(0, Math.min(1, (d.rssi_dbm + 90) / 70));
            const radius = r * (1 - norm);
            // Angle: hash the MAC to a stable position so devices don't
            // dance around. Add channel jitter for a touch of variety.
            const angle = macAngle(d.mac) + (d.channel * 0.05);
            const x = cx + Math.cos(angle) * radius;
            const y = cy + Math.sin(angle) * radius;

            const color = tag && tag.color ? tag.color : ACCENT;

            // Dot.
            ctx.fillStyle = color;
            ctx.beginPath();
            ctx.arc(x, y, 4 * (window.devicePixelRatio || 1), 0, Math.PI * 2);
            ctx.fill();

            // Hollow ring around the dot (radar "blip").
            ctx.strokeStyle = color;
            ctx.lineWidth = 1;
            ctx.beginPath();
            ctx.arc(x, y, 8 * (window.devicePixelRatio || 1), 0, Math.PI * 2);
            ctx.stroke();

            // Label.
            if (tag && tag.label) {
                ctx.fillStyle = color;
                ctx.font = "11px ui-monospace, monospace";
                ctx.textAlign = "left";
                ctx.fillText(tag.label, x + 10, y - 6);
            }
        }

        // Draw the trail back into the offscreen buffer so it persists.
        tctx.clearRect(0, 0, W, H);
        tctx.drawImage(this.canvas, 0, 0);
    };

    // Deterministic angle in [0, 2π) from a MAC string. We treat the
    // last 3 octets as a 24-bit hash so the angle is stable per device.
    function macAngle(mac) {
        const parts = mac.split(":");
        if (parts.length < 6) return 0;
        const v =
            (parseInt(parts[3], 16) << 16) |
            (parseInt(parts[4], 16) << 8) |
            parseInt(parts[5], 16);
        return (v / 0xffffff) * Math.PI * 2;
    }

    window.Radar = Radar;
})();