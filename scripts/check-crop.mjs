/**
 * 裁图管线的几何自检（纯 Node，无需测试框架）。
 *
 *   npm run check:crop
 *
 * 覆盖三条历史 bug 的回归防线：
 *   1. 锚点 ↔ 矩形 round-trip 误差（v21）
 *   2. type 步骤左扩过冲、元素右边缘被切（v25）
 *   3. 手动框选拖拽的对角漂移 / 新建框越界放大（v26）
 *
 * 用的是与运行时**完全相同的实现**（直接 import src/lib 下的真实代码），
 * 所以不存在"测试与实现各写一份、悄悄漂移"的问题。
 *
 * 需要 Node >= 22.6（--experimental-strip-types）。
 */

const A = await import(new URL('../src/lib/cropAnchor.ts', import.meta.url).href);
const D = await import(new URL('../src/lib/cropDrag.ts', import.meta.url).href);
const B = await import(new URL('../src/lib/cropBlank.ts', import.meta.url).href);

const { cropBoxFromAnchor, rectToCropAnchor, CROP_PAD } = A;
const { resizeRect, newRect, moveRect, MIN_CROP_SIZE } = D;

let failures = 0;
const fail = (msg) => {
  failures++;
  console.log('  ❌ ' + msg);
};

/** 可复现的伪随机（LCG），保证每次跑结果一致 */
function makeRng(seed) {
  let s = seed >>> 0;
  return () => ((s = (s * 1103515245 + 12345) & 0x7fffffff) / 0x7fffffff);
}

/* ═══ 1. 锚点 ↔ 矩形 round-trip ═══════════════════════ */

console.log('1. 锚点 ↔ 矩形 round-trip');
{
  const rng = makeRng(20260902);
  const types = ['click', 'type', 'scroll', undefined];
  let maxErr = 0, over2 = 0;
  const N = 20000;
  for (let i = 0; i < N; i++) {
    const t = types[i % types.length];
    const rect = {
      x: Math.floor(rng() * 1200),
      y: Math.floor(rng() * 700),
      width: 24 + Math.floor(rng() * 876),
      height: 24 + Math.floor(rng() * 576),
    };
    const anchor = rectToCropAnchor(rect, t);
    const back = cropBoxFromAnchor(anchor, t);
    const err =
      Math.abs(back.x - rect.x) +
      Math.abs(back.y - rect.y) +
      Math.abs(back.width - rect.width) +
      Math.abs(back.height - rect.height);
    if (err > maxErr) maxErr = err;
    if (err > 2) {
      over2++;
      if (over2 <= 5) fail(`err=${err} t=${t} ${JSON.stringify(rect)} -> ${JSON.stringify(back)}`);
    }
  }
  console.log(`   ${N} 例：maxErr=${maxErr}px，误差 >2px 的 ${over2} 例`);
  if (over2 > 0) fail(`round-trip 有 ${over2} 例误差超过 2px`);
}

/* ═══ 2. 四边留白对称（v25 防线）═════════════════════ */

console.log('2. 元素四边留白必须都 >= CROP_PAD');
{
  // 历史 bug：type 分支写成 `x - width/2 - extra`。框宽已含 extra，居中项
  // x - width/2 天然在左右各分到 extra/2，再左移 extra 会过冲 extra/2，
  // 使右侧留白变成 PAD - extra/2（元素宽 > 48px 即为负 → 右边缘被切）。
  let bad = 0;
  for (const cropW of [24, 48, 60, 120, 200, 300, 400, 600, 900]) {
    for (const t of ['click', 'type']) {
      const box = cropBoxFromAnchor({ x: 0, y: 0, cropW, cropH: 40 }, t);
      const left = -cropW / 2 - box.x;             // 元素左边缘 − 框左边缘
      const right = box.x + box.width - cropW / 2; // 框右边缘 − 元素右边缘
      if (left < CROP_PAD - 0.01 || right < CROP_PAD - 0.01) {
        bad++;
        fail(`t=${t} cropW=${cropW} left=${left} right=${right}（应 >= ${CROP_PAD}）`);
      }
    }
  }
  if (!bad) console.log(`   18 例全部通过（四边留白 >= ${CROP_PAD}px）`);
}

/* ═══ 3. 手动框选拖拽几何 ═════════════════════════════ */

console.log('3. 手动框选：resize 对角必须钉死');
{
  // 历史 bug：resizeRect 让宽高自由增长，再交给 clampRect 截断位置。
  // 位置被截断而宽高不变 → 对角跟着跑（实测 40.5% 的用例漂移，最大 1620px）。
  const anchors = (h) => {
    const a = {};
    if (h.includes('w')) a.right = true;
    else if (h.includes('e')) a.left = true;
    else { a.left = true; a.right = true; }
    if (h.includes('n')) a.bottom = true;
    else if (h.includes('s')) a.top = true;
    else { a.top = true; a.bottom = true; }
    return a;
  };
  const drift = (b, f, a) => {
    let d = 0;
    if (a.left) d = Math.max(d, Math.abs(f.x - b.x));
    if (a.right) d = Math.max(d, Math.abs(f.x + f.width - (b.x + b.width)));
    if (a.top) d = Math.max(d, Math.abs(f.y - b.y));
    if (a.bottom) d = Math.max(d, Math.abs(f.y + f.height - (b.y + b.height)));
    return d;
  };

  const HANDLES = ['nw', 'n', 'ne', 'e', 'se', 's', 'sw', 'w'];
  const DELTAS = [-2000, -900, -500, -310, -300, -160, -80, -20, 0, 20, 80, 160, 300, 500, 900, 2000];
  let maxD = 0, bad = 0, n = 0;
  for (const [W, H] of [[1920, 1080], [1366, 768], [800, 600]]) {
    for (const s of [
      { x: 500, y: 400, width: 300, height: 200 },
      { x: 100, y: 100, width: 200, height: 120 },
      { x: Math.min(1500, W - 380), y: Math.min(800, H - 240), width: 380, height: 240 },
    ]) {
      if (s.x < 0 || s.y < 0) continue;
      for (const h of HANDLES) {
        const a = anchors(h);
        for (const dx of DELTAS) {
          for (const dy of DELTAS) {
            const r = resizeRect(s, h, dx, dy, W, H);
            const d = drift(s, r, a);
            n++;
            if (d > maxD) maxD = d;
            if (d > 1) {
              bad++;
              if (bad <= 5) fail(`${W}x${H} 拖 ${h} d=(${dx},${dy}) 对角漂移 ${d}px`);
            }
          }
        }
      }
    }
  }
  console.log(`   ${n} 例：最大漂移 ${maxD}px，漂移 >1px 的 ${bad} 例`);
  if (bad > 0) fail(`resize 对角漂移共 ${bad} 例`);
}

console.log('4. 手动框选：新建框的两个端点必须都是角');
{
  const cl = (v, lo, hi) => Math.min(Math.max(v, lo), Math.max(lo, hi));
  const isCorner = (r, px, py) =>
    (Math.abs(r.x - px) <= 1 || Math.abs(r.x + r.width - px) <= 1) &&
    (Math.abs(r.y - py) <= 1 || Math.abs(r.y + r.height - py) <= 1);

  let bad = 0, n = 0;
  for (const [W, H] of [[1920, 1080], [1366, 768], [800, 600]]) {
    // 按下点必须在图内（拖拽起点在 stage 上，天然满足）
    const starts = [
      [Math.min(200, W - 1), Math.min(150, H - 1)],
      [Math.round(W * 0.5), Math.round(H * 0.5)],
      [Math.round(W * 0.9), Math.round(H * 0.9)],
      [10, 10],
    ];
    for (const [ax, ay] of starts) {
      for (const [bx, by] of [[-500, -400], [-100, 0], [0, 0], [300, 200], [9999, 9999], [W, H], [ax + 30, ay + 30]]) {
        const r = newRect(ax, ay, bx, by, W, H);
        n++;
        const px = cl(bx, 0, W), py = cl(by, 0, H);
        const okPointer = isCorner(r, px, py);
        // 指针被 clamp 后包围盒不足最小边长时实现会补足，此时按下点可不成角
        const needPad = Math.abs(px - ax) < MIN_CROP_SIZE - 0.5 || Math.abs(py - ay) < MIN_CROP_SIZE - 0.5;
        const okStart = needPad || isCorner(r, ax, ay);
        if (!okPointer || !okStart) {
          bad++;
          if (bad <= 5) fail(`${W}x${H} 按下(${ax},${ay}) 拖到(${bx},${by}) -> ${JSON.stringify(r)}`);
        }
      }
    }
  }
  console.log(`   ${n} 例：违反 ${bad} 例`);
  if (bad > 0) fail(`新建框端点成角共 ${bad} 例违规`);
}

console.log('5. 手动框选：平移尺寸恒定 + 任意输入不越界');
{
  let bad = 0, n = 0;
  for (const [W, H] of [[1920, 1080], [800, 600]]) {
    for (const o of [{ x: 100, y: 80, width: 300, height: 200 }, { x: Math.max(0, W - 620), y: Math.max(0, H - 340), width: 600, height: 300 }]) {
      for (const dx of [-9999, -500, -50, 0, 50, 500, 9999]) {
        for (const dy of [-9999, -500, -50, 0, 50, 500, 9999]) {
          const r = moveRect(o, dx, dy, W, H);
          n++;
          const sizeOk = r.width === o.width && r.height === o.height;
          const inOk = r.x >= 0 && r.y >= 0 && r.x + r.width <= W && r.y + r.height <= H;
          if (!sizeOk || !inOk) {
            bad++;
            if (bad <= 5) fail(`move d=(${dx},${dy}) -> ${JSON.stringify(r)} sizeOk=${sizeOk} inOk=${inOk}`);
          }
        }
      }
    }
  }

  const rng = makeRng(42);
  const HANDLES = ['nw', 'n', 'ne', 'e', 'se', 's', 'sw', 'w'];
  let safeBad = 0;
  for (let i = 0; i < 20000; i++) {
    const W = Math.max(1, Math.round(rng() * 2000));
    const H = Math.max(1, Math.round(rng() * 2000));
    const o = { x: rng() * W, y: rng() * H, width: rng() * W, height: rng() * H };
    const h = HANDLES[Math.floor(rng() * 8)];
    const dx = (rng() - 0.5) * 4000;
    const dy = (rng() - 0.5) * 4000;
    n++;
    for (const f of [resizeRect(o, h, dx, dy, W, H), newRect(o.x, o.y, o.x + dx, o.y + dy, W, H), moveRect(o, dx, dy, W, H)]) {
      const ok =
        f.x >= 0 && f.y >= 0 &&
        f.x + f.width <= Math.max(W, MIN_CROP_SIZE) + 0.5 &&
        f.y + f.height <= Math.max(H, MIN_CROP_SIZE) + 0.5 &&
        f.width >= Math.min(MIN_CROP_SIZE, W) - 0.5 &&
        f.height >= Math.min(MIN_CROP_SIZE, H) - 0.5 &&
        Number.isFinite(f.x) && Number.isFinite(f.y) && Number.isFinite(f.width) && Number.isFinite(f.height);
      if (!ok) {
        safeBad++;
        if (safeBad <= 5) fail(`W=${W} H=${H} h=${h} d=(${dx.toFixed(0)},${dy.toFixed(0)}) -> ${JSON.stringify(f)}`);
      }
    }
  }
  bad += safeBad;
  console.log(`   ${n} 例（含 2 万次随机 × 3 操作）：违反 ${bad} 例`);
  if (bad > 0) fail(`平移/安全不变式共 ${bad} 例违规`);
}

/* ═══ 6. 跨端黄金值（与 Rust 导出端共用同一组数）══════ */

console.log('6. 跨端黄金值：编辑器与导出必须逐像素一致');
{
  // 同一套裁图算法在仓库里有 4 份实现（详见 src-tauri/src/export/commands.rs
  // 的 export_crop_box 文档注释）。前端 2 份与导出端常量完全一致（PAD=8），
  // 因此必须算出同样的矩形 —— 否则编辑器里看到的图和导出的图不是同一张。
  //
  // ⚠️ 下面每个数字都同时硬编码在 `src-tauri/src/export/commands.rs` 的
  //    `export_crop_box_golden_*` 测试里。改任一端都必须同步改两处。
  //
  // 场景：1920x1080 截图，元素矩形中心 (960,540)
  //   type  300x40：extra = round(300/3) = 100，w = 300+16+100 = 416，x = 960-208-50 = 702
  //   click 300x40：w = 316，x = 960-158 = 802
  const GOLDEN = [
    ['type  300x40', { x: 960, y: 540, cropW: 300, cropH: 40 }, 'type', [702, 512, 416, 56]],
    ['click 300x40', { x: 960, y: 540, cropW: 300, cropH: 40 }, 'click', [802, 512, 316, 56]],
    ['type   24x40', { x: 960, y: 540, cropW: 24, cropH: 40 }, 'type', [932, 512, 48, 56]],
    ['type  900x40', { x: 960, y: 540, cropW: 900, cropH: 40 }, 'type', [202, 512, 1216, 56]],
  ];
  let bad = 0;
  for (const [name, anchor, t, [gx, gy, gw, gh]] of GOLDEN) {
    const b = cropBoxFromAnchor(anchor, t);
    const got = [b.x, b.y, b.width, b.height];
    if (got.some((v, i) => v !== [gx, gy, gw, gh][i])) {
      bad++;
      fail(`${name} 期望 [${gx},${gy},${gw},${gh}]，实得 [${got}]`);
    }
  }
  if (!bad) {
    console.log(`   ${GOLDEN.length} 组与 Rust 导出端一致`);
    console.log('   （AI 管线 PAD=24/MIN=280，常量不同，黄金值另见其 cargo test）');
  }
}

/* ═══ 7. 空白裁剪守卫（isUniformCropPixels）═════════════ */

console.log('7. 空白裁剪守卫：纯色区域裁剪必须回退整图（三端同步）');
{
  // 黄金值：与 Rust export/commands.rs::BLANK_CROP_DELTA 锁死（is_blank_crop_basic 测试）
  if (B.BLANK_CROP_DELTA !== 24) fail(`BLANK_CROP_DELTA 期望 24，实得 ${B.BLANK_CROP_DELTA}`);

  const mk = (w, h, fill) => {
    const a = new Uint8ClampedArray(w * h * 4);
    for (let i = 0; i < w * h; i++) {
      a[i * 4] = fill[0]; a[i * 4 + 1] = fill[1]; a[i * 4 + 2] = fill[2]; a[i * 4 + 3] = 255;
    }
    return a;
  };

  // ① 纯灰（真实案例：WPS 加载中录到的空白区）→ 判空白
  if (!B.isUniformCropPixels(mk(576, 148, [214, 214, 214]), 576, 148))
    fail('纯灰区域应判为空白（真实案例回归）');
  if (!B.isUniformCropPixels(mk(1920, 1080, [255, 255, 255]), 1920, 1080))
    fail('纯白大图应判为空白');
  if (!B.isUniformCropPixels(mk(300, 200, [10, 20, 30]), 300, 200))
    fail('纯深色图应判为空白（与亮度无关，只看极差）');

  // ② ±6 的 JPEG 式噪声（LCG 可复现）→ 仍应判空白（阈值 24 要有足够余量）
  {
    const rng = makeRng(20260905);
    const w = 400, h = 300;
    const a = mk(w, h, [214, 214, 214]);
    for (let i = 0; i < w * h; i++) {
      const n = Math.round((rng() - 0.5) * 12); // ±6
      a[i * 4] += n; a[i * 4 + 1] += n; a[i * 4 + 2] += n;
    }
    if (!B.isUniformCropPixels(a, w, h)) fail('±6 噪声的平坦区域应判为空白（JPEG 噪声余量不足）');
  }

  // ③ 含深色块（可见内容）→ 必须判非空白；块放在各角落也要被采到
  {
    const w = 400, h = 300;
    const spots = [[196, 146], [4, 4], [w - 13, h - 13], [4, h - 13], [w - 13, 4]];
    for (const [sx, sy] of spots) {
      const a = mk(w, h, [214, 214, 214]);
      for (let y = sy; y < sy + 8; y++)
        for (let x = sx; x < sx + 8; x++) {
          const i = (y * w + x) * 4;
          a[i] = 20; a[i + 1] = 20; a[i + 2] = 20;
        }
      if (B.isUniformCropPixels(a, w, h)) fail(`(${sx},${sy}) 处的深色块应判为非空白`);
    }
  }

  // ④ 边界值：极差恰好 24 → 空白；25 → 非空白（锁阈值语义）
  {
    const w = 64, h = 64;
    const a = mk(w, h, [100, 100, 100]);
    a[0] = 124; // 极差 24
    if (!B.isUniformCropPixels(a, w, h)) fail('通道极差 = 24 应判空白（<= 阈值）');
    const b = mk(w, h, [100, 100, 100]);
    b[0] = 125; // 极差 25
    if (B.isUniformCropPixels(b, w, h)) fail('通道极差 = 25 应判非空白（> 阈值）');
  }

  if (!failures) console.log('   黄金值 Δ=24 / 噪声余量 / 角落采样 / 边界语义 全部通过');
}

/* ═══ 结论 ════════════════════════════════════════════ */

console.log('');
if (failures) {
  console.log(`❌ 共 ${failures} 项失败`);
  process.exit(1);
}
console.log('✅ 裁图几何全部通过');
