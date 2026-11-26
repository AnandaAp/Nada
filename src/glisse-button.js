(function () {
  "use strict";

  const POINTER = { grow: 6, press: [417.6, 29.2], release: [230, 23.7] };
  const TOUCH = { grow: 15, press: [515, 27.3], release: [250, 11.8] };

  const moving = new Set();
  let frame = 0;
  let last = 0;

  function prefersReducedMotion() {
    return typeof matchMedia === "function" && matchMedia("(prefers-reduced-motion: reduce)").matches;
  }

  function advance(s, dt) {
    const x = s.value - s.target;
    const v = s.velocity;
    const half = s.damping / 2;
    const discriminant = half * half - s.stiffness;

    if (Math.abs(discriminant) < 1e-4) {
      const decay = Math.exp(-half * dt);
      const b = v + half * x;
      s.value = s.target + decay * (x + b * dt);
      s.velocity = decay * (v - half * b * dt);
    } else if (discriminant > 0) {
      const root = Math.sqrt(discriminant);
      const r1 = -half + root;
      const r2 = -half - root;
      const a = (v - r2 * x) / (r1 - r2);
      const b = x - a;
      s.value = s.target + a * Math.exp(r1 * dt) + b * Math.exp(r2 * dt);
      s.velocity = a * r1 * Math.exp(r1 * dt) + b * r2 * Math.exp(r2 * dt);
    } else {
      const omega = Math.sqrt(-discriminant);
      const decay = Math.exp(-half * dt);
      const cos = Math.cos(omega * dt);
      const sin = Math.sin(omega * dt);
      s.value = s.target + decay * (x * cos + ((v + half * x) / omega) * sin);
      s.velocity = decay * (v * cos - ((half * v + s.stiffness * x) / omega) * sin);
    }
  }

  function tick(time) {
    frame = 0;
    const dt = Math.min(0.1, Math.max(0, (time - last) / 1000));
    last = time;
    for (const s of moving) {
      advance(s, dt);
      if (Math.abs(s.value - s.target) < 1e-4 && Math.abs(s.velocity) < 1e-3) {
        s.value = s.target;
        s.velocity = 0;
        moving.delete(s);
      }
      s.paint(s.value);
    }
    if (moving.size) frame = requestAnimationFrame(tick);
  }

  function createSpring(value, stiffness, damping, paint) {
    const s = { value, target: value, velocity: 0, stiffness, damping, paint };
    return {
      paint() { paint(s.value); },
      set(target, instant) {
        s.target = target;
        if (instant || document.hidden || prefersReducedMotion()) {
          s.value = target;
          s.velocity = 0;
          moving.delete(s);
          paint(target);
          return;
        }
        moving.add(s);
        if (!frame) {
          last = Number((document.timeline && document.timeline.currentTime) || performance.now());
          frame = requestAnimationFrame(tick);
        }
      },
      tune(nextStiffness, nextDamping) {
        s.stiffness = nextStiffness;
        s.damping = nextDamping;
      },
      stop() {
        moving.delete(s);
      },
    };
  }

  const clamp = (value, min, max) => Math.min(max, Math.max(min, value));

  function distance(x, y, halfWidth, halfHeight, radius) {
    const qx = Math.abs(x) - halfWidth + radius;
    const qy = Math.abs(y) - halfHeight + radius;
    return Math.hypot(Math.max(qx, 0), Math.max(qy, 0)) + Math.min(Math.max(qx, qy), 0) - radius;
  }

  function shapeOf(element, style) {
    const width = element.offsetWidth;
    const height = element.offsetHeight;
    if (!width || !height) return null;
    const corner = style.borderTopLeftRadius || "0px";
    const radius = Math.min(
      width / 2,
      height / 2,
      corner.endsWith("%") ? (Math.min(width, height) * parseFloat(corner)) / 100 : parseFloat(corner) || 0,
    );
    return { width, height, radius };
  }

  const RIM_WIDTH = 1.22;
  const EDGE_WIDTH = 0.5;
  const rims = new Map();
  let palette = null;

  function rimMap(shape, shine, density, edge) {
    const { width, height, radius } = shape;
    const columns = Math.max(1, Math.round(width * density));
    const rows = Math.max(1, Math.round(height * density));
    const data = new Uint8ClampedArray(columns * rows * 4);
    const halfWidth = width / 2;
    const halfHeight = height / 2;
    const corner = clamp(radius, 0, Math.min(halfWidth, halfHeight));
    const half = 0.5 / density;

    for (let row = 0; row < rows; row++) {
      for (let column = 0; column < columns; column++) {
        const x = ((column + 0.5) / columns) * width - halfWidth;
        const y = ((row + 0.5) / rows) * height - halfHeight;
        const depth = -distance(x, y, halfWidth, halfHeight, corner);
        if (depth < -half || depth > 2 * RIM_WIDTH) continue;

        const qx = Math.abs(x) - halfWidth + corner;
        const qy = Math.abs(y) - halfHeight + corner;
        let nx = 0;
        let ny = 0;
        if (qx > 0 && qy > 0) {
          const length = Math.hypot(qx, qy);
          nx = (Math.sign(x) * qx) / length;
          ny = (Math.sign(y) * qy) / length;
        } else if (qx > qy) {
          nx = Math.sign(x);
        } else {
          ny = Math.sign(y);
        }

        const facing = Math.abs(nx + ny);
        const along = Math.max(0, depth) / (RIM_WIDTH * Math.sqrt(Math.max(1, facing)));
        const cover = clamp((depth + half) * density, 0, 1);
        const lined = clamp((Math.min(EDGE_WIDTH, depth + half) - Math.max(0, depth - half)) * density, 0, 1);
        const light = along < 1 ? shine * Math.pow(Math.min(1, facing), 1.5) * (1 - Math.pow(along, 1.5)) * cover : 0;
        const line = edge[3] * lined * (1 - light);
        const alpha = light + line;
        if (alpha <= 0) continue;

        const i = (row * columns + column) * 4;
        for (let channel = 0; channel < 3; channel++) {
          data[i + channel] = Math.round((255 * light + edge[channel] * line) / alpha);
        }
        data[i + 3] = Math.round(255 * alpha);
      }
    }
    return { columns, rows, data };
  }

  function parseColour(context, colour) {
    context.fillStyle = "#0000";
    context.fillStyle = colour;
    const value = String(context.fillStyle);
    if (value.startsWith("#")) {
      const r = parseInt(value.slice(1, 3), 16);
      const g = parseInt(value.slice(3, 5), 16);
      const b = parseInt(value.slice(5, 7), 16);
      return [r, g, b, 1];
    }
    const parts = (value.match(/[\d.]+/g) || []).map(Number);
    return [parts[0] || 0, parts[1] || 0, parts[2] || 0, parts[3] === undefined ? 1 : parts[3]];
  }

  function paintRim(element) {
    if (typeof getComputedStyle !== "function") return;
    const style = getComputedStyle(element);
    const shape = shapeOf(element, style);
    if (!palette) {
      const c = document.createElement("canvas");
      palette = c.getContext ? c.getContext("2d") : null;
    }
    if (!shape || !palette) return;

    const tuned = parseFloat(style.getPropertyValue("--lg-rim-shine"));
    const shine = Number.isFinite(tuned) ? clamp(tuned, 0, 1) : 0.16;
    const edge = parseColour(palette, style.getPropertyValue("--lg-edge").trim() || "transparent");
    const density = Math.min(window.devicePixelRatio || 1, 3);
    const key = [shape.width, shape.height, shape.radius, shine, edge.join(","), density].join("|");

    let url = rims.get(key);
    if (!url) {
      const { columns, rows, data } = rimMap(shape, shine, density, edge);
      const canvas = document.createElement("canvas");
      canvas.width = columns;
      canvas.height = rows;
      const context = canvas.getContext("2d");
      if (!context) return;
      const image = context.createImageData(columns, rows);
      image.data.set(data);
      context.putImageData(image, 0, 0);
      url = 'url("' + canvas.toDataURL() + '")';
      rims.set(key, url);
    }
    element.style.setProperty("--lg-rim", url);
    element.dataset.rim = "";
  }

  const attached = new WeakSet();

  function attachGlisseButton(el) {
    if (!el || attached.has(el)) return;
    attached.add(el);

    const value = [1, 1, 0, 0];
    const paint = () => {
      el.style.scale = value[0] + " " + value[1];
      el.style.translate = value[2] + "px " + value[3] + "px";
    };
    const springs = value.map((initial, i) =>
      createSpring(initial, POINTER.release[0], POINTER.release[1], (next) => {
        value[i] = next;
        paint();
      }),
    );

    let pointer = null;

    const target = (next, tuning) => {
      const stiffness = tuning[0];
      const damping = tuning[1];
      springs.forEach((spring, i) => {
        spring.tune(stiffness, damping);
        spring.set(next[i] === undefined ? 0 : next[i]);
      });
    };

    const track = (event) => {
      if (!pointer) return;
      const { box, feel } = pointer;
      if (box.width > 0 && box.height > 0) {
        el.style.setProperty("--lg-x", ((event.clientX - box.left) / box.width) * 100 + "%");
        el.style.setProperty("--lg-y", ((event.clientY - box.top) / box.height) * 100 + "%");
      }
      const width = box.width || 40;
      const height = box.height || 40;
      const base = 1 + feel.grow / Math.max(width, height);
      const aspect = width / height;
      const dx = (event.clientX - pointer.x) / aspect;
      const dy = event.clientY - pointer.y;
      const length = Math.hypot(dx, dy);
      if (!length) return target([base, base, 0, 0], feel.press);

      const nx = dx / length;
      const ny = dy / length;
      const rubber = 1 - 1 / (1 + length / height / (5 * aspect));
      const stretch = 1 + (6 / height) * rubber * Math.abs(Math.abs(nx) - Math.abs(ny));
      const horizontal = Math.abs(nx) > Math.abs(ny);
      target(
        [
          base * (horizontal ? stretch : 1 / stretch),
          base * (horizontal ? 1 / stretch : stretch),
          nx * 8 * rubber,
          ny * 8 * rubber,
        ],
        feel.press,
      );
    };

    const move = (event) => {
      if (pointer && event.pointerId === pointer.id) track(event);
    };

    const end = (event) => {
      if (event && (!pointer || event.pointerId !== pointer.id)) return;
      const feel = (pointer && pointer.feel) || POINTER;
      pointer = null;
      delete el.dataset.pressed;
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", end);
      window.removeEventListener("pointercancel", end);
      target([1, 1, 0, 0], feel.release);
    };

    const down = (event) => {
      if (!event.isPrimary || event.button !== 0 || pointer || el.disabled) return;
      pointer = {
        id: event.pointerId,
        x: event.clientX,
        y: event.clientY,
        box: el.getBoundingClientRect(),
        feel: event.pointerType === "mouse" ? POINTER : TOUCH,
      };
      el.dataset.pressed = "";
      window.addEventListener("pointermove", move);
      window.addEventListener("pointerup", end);
      window.addEventListener("pointercancel", end);
      track(event);
    };

    el.addEventListener("pointerdown", down);
    window.addEventListener("blur", () => end());

    paintRim(el);
    if (typeof ResizeObserver === "function") {
      const ro = new ResizeObserver(() => paintRim(el));
      ro.observe(el);
    }
  }

  function scan(root) {
    if (!root || !root.querySelectorAll) return;
    if (root.classList && root.classList.contains("lg-button")) attachGlisseButton(root);
    root.querySelectorAll(".lg-button").forEach(attachGlisseButton);
  }

  function init() {
    scan(document);
    if (typeof MutationObserver === "function" && document.body) {
      const mo = new MutationObserver((mutations) => {
        for (const m of mutations) {
          for (const node of m.addedNodes) {
            if (node.nodeType === 1) scan(node);
          }
        }
      });
      mo.observe(document.body, { childList: true, subtree: true });
    }
  }

  if (typeof window !== "undefined") {
    window.glisseAttachButton = attachGlisseButton;
    if (document.readyState === "loading") {
      document.addEventListener("DOMContentLoaded", init);
    } else {
      init();
    }
  }
})();
