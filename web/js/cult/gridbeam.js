// GridBeam：工作台唯一 canvas 背景（Cult UI GridBeam 的 vanilla 移植）。
// 网格 + 对角扫掠光束 + 节点脉冲；颜色经探针元素从 CSS 变量解析（亮暗主题跟随）；
// reduce-motion 只绘制静态单帧；隐藏或页面不可见时暂停 rAF。

const TAU = Math.PI * 2;
const GAP = 46;
const CYCLE_MS = 9000; // 与 --dur-beam 同周期

// 计算色可能是 rgb()/oklab() 等任意 CSS Color 4 形态：统一经 1x1 画布标准化为 rgba 字节。
function resolvedColor(swatchCtx, probe, cssColor, fallback) {
  probe.style.color = cssColor;
  const computed = getComputedStyle(probe).color;
  probe.style.color = '';
  swatchCtx.fillStyle = `rgba(${fallback.r},${fallback.g},${fallback.b},1)`;
  swatchCtx.fillStyle = computed; // 无效赋值会被静默忽略，从而保留回退色
  swatchCtx.clearRect(0, 0, 1, 1);
  swatchCtx.fillRect(0, 0, 1, 1);
  const d = swatchCtx.getImageData(0, 0, 1, 1).data;
  return { r: d[0], g: d[1], b: d[2], a: d[3] / 255 };
}

export function mountGridBeam(canvas) {
  const ctx = canvas.getContext('2d');
  const reducedQuery = window.matchMedia('(prefers-reduced-motion: reduce)');
  const probe = document.createElement('span');
  probe.hidden = true;
  document.body.append(probe);
  const swatch = document.createElement('canvas');
  swatch.width = 1;
  swatch.height = 1;
  const swatchCtx = swatch.getContext('2d', { willReadFrequently: true });

  const host = canvas.parentElement;
  let width = 0;
  let height = 0;
  let dpr = 1;
  let active = false;
  let running = false;
  let raf = 0;
  const t0 = performance.now();
  let nodes = [];
  let palette = null;

  const rgba = (c, alpha) => `rgba(${c.r},${c.g},${c.b},${Math.max(0, Math.min(1, c.a * alpha)).toFixed(3)})`;

  function paletteNow() {
    return {
      line: resolvedColor(swatchCtx, probe, 'var(--border-strong)', { r: 128, g: 128, b: 128, a: 1 }),
      beam: resolvedColor(swatchCtx, probe, 'var(--brand)', { r: 168, g: 217, b: 79, a: 1 }),
    };
  }

  function seedNodes() {
    nodes = [];
    const cols = Math.ceil(width / GAP);
    const rows = Math.ceil(height / GAP);
    const count = Math.min(14, Math.max(6, Math.round((width * height) / 90000)));
    let seed = 7;
    const rand = () => ((seed = (seed * 16807) % 2147483647) / 2147483647);
    for (let i = 0; i < count; i++) {
      nodes.push({
        x: Math.round(rand() * cols) * GAP + (width % GAP) / 2,
        y: (Math.round((height * 0.28) / GAP) + Math.round(rand() * rows)) * GAP,
        phase: rand() * TAU,
      });
    }
  }

  function measure() {
    // 每次以宿主当前客户区为准（部分内嵌 webview 的 ResizeObserver 初始回调不可靠）。
    const w = host.clientWidth;
    const h = host.clientHeight;
    dpr = Math.min(window.devicePixelRatio || 1, 2);
    width = Math.max(1, Math.round(w));
    height = Math.max(1, Math.round(h));
    const bw = Math.round(width * dpr);
    const bh = Math.round(height * dpr);
    if (canvas.width !== bw || canvas.height !== bh) {
      canvas.width = bw;
      canvas.height = bh;
      seedNodes();
    }
  }

  function draw(now) {
    const t = now - t0;
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.clearRect(0, 0, width, height);

    // 横线向下渐显（地面感），竖线整幅均匀。
    const vertical = ctx.createLinearGradient(0, height * 0.2, 0, height);
    vertical.addColorStop(0, rgba(palette.line, 0));
    vertical.addColorStop(1, rgba(palette.line, 0.5));
    ctx.lineWidth = 1;
    ctx.strokeStyle = vertical;
    ctx.beginPath();
    for (let y = Math.ceil((height * 0.2) / GAP) * GAP; y < height + GAP; y += GAP) {
      ctx.moveTo(0, y + 0.5);
      ctx.lineTo(width, y + 0.5);
    }
    ctx.stroke();
    ctx.strokeStyle = rgba(palette.line, 0.3);
    ctx.beginPath();
    for (let x = (width % GAP) / 2; x < width + GAP; x += GAP) {
      ctx.moveTo(x + 0.5, height * 0.16);
      ctx.lineTo(x + 0.5, height);
    }
    ctx.stroke();

    // 光束：沿对角线扫掠，一周期 9s；带内高斯式渐隐。
    const p = (((t % CYCLE_MS) + CYCLE_MS) % CYCLE_MS) / CYCLE_MS;
    ctx.save();
    ctx.translate(width * (p * 1.6 - 0.3), 0);
    ctx.rotate(0.42);
    const bw = 220;
    const beam = ctx.createLinearGradient(-bw, 0, bw, 0);
    beam.addColorStop(0, rgba(palette.beam, 0));
    beam.addColorStop(0.5, rgba(palette.beam, 0.1));
    beam.addColorStop(1, rgba(palette.beam, 0));
    ctx.fillStyle = beam;
    ctx.fillRect(-bw, -height, bw * 2, height * 4);
    ctx.restore();

    // 节点脉冲：仅透明度呼吸。
    for (const node of nodes) {
      const pulse = 0.5 + 0.5 * Math.sin(t / 1400 + node.phase);
      ctx.fillStyle = rgba(palette.beam, 0.06 + 0.3 * pulse);
      ctx.fillRect(node.x - 1.5, node.y - 1.5, 3, 3);
    }
  }

  function paintStatic() {
    if (!palette) palette = paletteNow();
    draw(t0 + CYCLE_MS * 0.35); // 光束停在画面中部的单帧
  }

  let watchdog = 0;
  let lastTick = 0;

  function stopLoop() {
    cancelAnimationFrame(raf);
    running = false;
  }

  function startLoop() {
    if (running || reducedQuery.matches) return;
    if (!palette) palette = paletteNow();
    running = true;
    lastTick = performance.now();
    raf = requestAnimationFrame(frame);
  }

  function frame(now) {
    if (!active || document.hidden) { stopLoop(); return; }
    lastTick = now;
    measure();
    draw(now);
    raf = requestAnimationFrame(frame);
  }

  // 挂载即启动的自愈通道：rAF 正常时它只做尺寸校正、不插手绘制（lastTick 新鲜）；
  // rAF 被内嵌 webview 暂停时按 4fps 接管绘制。不依赖任何调用顺序。
  watchdog = setInterval(() => {
    if (!active || reducedQuery.matches) return;
    measure();
    const now = performance.now();
    if (now - lastTick >= 250) { lastTick = now; draw(now); }
  }, 250);

  const resize = new ResizeObserver(() => {
    measure();
    if (active && reducedQuery.matches) paintStatic();
  });
  resize.observe(host);
  const theme = new MutationObserver(() => {
    palette = paletteNow();
    if (active && reducedQuery.matches) paintStatic();
  });
  theme.observe(document.body, { attributes: true, attributeFilter: ['data-theme'] });
  reducedQuery.addEventListener('change', () => {
    if (!active) return;
    if (reducedQuery.matches) { stopLoop(); paintStatic(); }
    else startLoop();
  });
  document.addEventListener('visibilitychange', () => {
    if (active && !document.hidden) startLoop();
  });

  measure();
  return {
  setActive(value) {
    active = Boolean(value);
    canvas.hidden = !active;
    if (!active) { stopLoop(); return; }
    measure();
    palette = paletteNow();
    if (reducedQuery.matches) paintStatic();
    else startLoop();
  },
  destroy() {
    resize.disconnect();
    theme.disconnect();
    if (watchdog) { clearInterval(watchdog); watchdog = 0; }
    probe.remove();
    stopLoop();
  },
  };
}
