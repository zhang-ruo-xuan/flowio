/**
 * 标注弹窗缩放的量化校验。
 *
 *   node --experimental-strip-types scripts/check-annotate-fit.mjs
 *
 * 不变式：**canvas 的 CSS 尺寸必须完整落在内容区内**——
 * 标注弹窗的 `.annotation-canvas-area` 是 `overflow: hidden` + `flex-shrink: 0`，
 * 一旦溢出就是被对称裁掉，用户看到的图缺一条边，且被裁掉的区域画不到标注。
 *
 * 需要 Node >= 22.6。
 */

const F = await import(new URL('../src/lib/fitBox.ts', import.meta.url).href);
const { containScale, fitDisplaySize, contentArea } = F;

let failures = 0;
const fail = (m) => {
  failures++;
  console.log('  ❌ ' + m);
};

/* ── 布局常量（与 src/styles.css 一致）─────────────────────
 * .annotation-overlay  : position fixed; inset 0; flex column
 * .annotation-toolbar  : height 56px
 * .annotation-canvas-area : flex 1; padding 40px; overflow hidden
 */
const TOOLBAR_H = 56;
const AREA_PAD = 40; // 四周各 40

/** 旧实现：直接拿 window 尺寸当可用区（padding 从未扣除） */
function oldScale(vw, vh, imgW, imgH) {
  const maxW = Math.min(vw * 0.995, imgW);
  const maxH = vh - TOOLBAR_H;
  return Math.min(maxW / imgW, maxH / imgH, 1);
}

/** 新实现：先扣掉 padding，再等比 contain */
function newScale(vw, vh, imgW, imgH) {
  const box = contentArea(vw, vh - TOOLBAR_H, AREA_PAD, AREA_PAD);
  return containScale({ availW: box.availW, availH: box.availH, imgW, imgH });
}

// 常见屏幕 × 常见截图分辨率
const VIEWPORTS = [
  [1280, 720],
  [1366, 768],
  [1440, 900],
  [1600, 900],
  [1920, 1080],
  [2560, 1440],
  [3440, 1440],
  [3840, 2160],
];
const SHOTS = [
  [1280, 720],
  [1366, 768],
  [1440, 900],
  [1600, 900],
  [1920, 1080],
  [2560, 1440],
  [3440, 1440],
  [3840, 2160],
];

/* ═══ 1. 溢出量化：旧 vs 新 ═══════════════════════════════ */

console.log('1. 标注弹窗：canvas 必须完整落在内容区内（不能被裁）');
{
  let oldBad = 0, newBad = 0, n = 0;
  let oldMaxX = 0, oldMaxY = 0;
  const samples = [];
  for (const [vw, vh] of VIEWPORTS) {
    for (const [iw, ih] of SHOTS) {
      n++;
      // 内容区（真实可用）：宽度扣左右 padding，高度扣工具栏 + 上下 padding
      const availW = vw - AREA_PAD * 2;
      const availH = vh - TOOLBAR_H - AREA_PAD * 2;

      const so = oldScale(vw, vh, iw, ih);
      const { w: ow, h: oh } = fitDisplaySize({ availW: 1, availH: 1, imgW: iw, imgH: ih });
      // 用旧 scale 的实际显示尺寸
      const odW = iw * so, odH = ih * so;
      const overX = odW - availW, overY = odH - availH;
      const over = Math.max(overX, overY);
      if (over > 0.5) {
        oldBad++;
        oldMaxX = Math.max(oldMaxX, overX);
        oldMaxY = Math.max(oldMaxY, overY);
        if (samples.length < 6) {
          samples.push(
            `     屏幕 ${vw}x${vh} + 截图 ${iw}x${ih}：scale=${so.toFixed(3)} → ` +
              `canvas ${odW.toFixed(0)}x${odH.toFixed(0)}，可用 ${availW}x${availH}` +
              ` → 横向超 ${overX.toFixed(0)}px / 纵向超 ${overY.toFixed(0)}px`
          );
        }
      }

      const sn = newScale(vw, vh, iw, ih);
      const ndW = iw * sn, ndH = ih * sn;
      if (ndW - availW > 0.5 || ndH - availH > 0.5) {
        newBad++;
        fail(`新实现仍溢出 ${vw}x${vh} + ${iw}x${ih}`);
      }
    }
  }
  console.log(`   ${n} 种组合`);
  console.log(
    `   旧实现：溢出 ${oldBad}/${n} 例（横向最大超 ${oldMaxX.toFixed(0)}px / 纵向最大超 ${oldMaxY.toFixed(0)}px）`
  );
  console.log(`   新实现：溢出 ${newBad}/${n} 例`);
  if (samples.length) {
    console.log('   典型：');
    for (const s of samples) console.log(s);
  }
  if (newBad > 0) fail(`新实现仍有 ${newBad} 例溢出`);
}

/* ═══ 2. 缩放后必须保持宽高比（图不能变形）════════════════ */

console.log('2. 缩放后宽高比必须与图一致（不变形）');
{
  let bad = 0, n = 0, maxDrift = 0;
  for (const [vw, vh] of VIEWPORTS) {
    for (const [iw, ih] of SHOTS) {
      const box = contentArea(vw, vh - TOOLBAR_H, AREA_PAD, AREA_PAD);
      const { w, h } = fitDisplaySize({ availW: box.availW, availH: box.availH, imgW: iw, imgH: ih });
      const drift = Math.abs(w / h - iw / ih);
      n++;
      if (drift > maxDrift) maxDrift = drift;
      if (drift > 1e-9) {
        bad++;
        if (bad <= 3) fail(`${vw}x${vh} + ${iw}x${ih} 宽高比漂移 ${drift}`);
      }
    }
  }
  console.log(`   ${n} 例：最大宽高比漂移 ${maxDrift.toExponential(2)}，违反 ${bad} 例`);
  if (bad > 0) fail(`宽高比漂移共 ${bad} 例`);
}

/* ═══ 3. containScale 的边界与非法输入 ════════════════════ */

console.log('3. containScale 边界：永不放大、非法输入不崩');
{
  // 永不放大
  for (const [iw, ih] of [[100, 100], [320, 240], [800, 600]]) {
    const s = containScale({ availW: 99999, availH: 99999, imgW: iw, imgH: ih });
    if (s !== 1) fail(`图 ${iw}x${ih} 在超大容器里被放大到 ${s}（应恒为 1）`);
  }
  // 等比缩小
  {
    const s = containScale({ availW: 500, availH: 500, imgW: 1000, imgH: 500 });
    if (Math.abs(s - 0.5) > 1e-12) fail(`1000x500 放进 500x500 应得 0.5，实得 ${s}`);
  }
  // 非法输入
  const bads = [
    { availW: 0, availH: 0, imgW: 100, imgH: 100 },
    { availW: 100, availH: 100, imgW: 0, imgH: 100 },
    { availW: 100, availH: 100, imgW: 100, imgH: 0 },
    { availW: -10, availH: -10, imgW: 100, imgH: 100 },
  ];
  for (const b of bads) {
    const s = containScale(b);
    if (!Number.isFinite(s) || s <= 0) fail(`非法输入 ${JSON.stringify(b)} → ${s}（应回落 1）`);
  }
  // 极端窄容器
  {
    const s = containScale({ availW: 1, availH: 1, imgW: 4000, imgH: 3000 });
    if (!(s > 0)) fail(`极窄容器应得到正的极小 scale，实得 ${s}`);
  }
  console.log('   全部通过（不放大 / 等比 / 非法输入回落 1 / 极端容器不塌缩）');
}

/* ═══ 4. contentArea 减法正确 ═════════════════════════════ */

console.log('4. contentArea 扣除四周留白');
{
  const cases = [
    [1920, 1024, 40, 40, 1840, 944],
    [1366, 712, 40, 40, 1286, 632],
    [0, 0, 40, 40, 0, 0],
    [50, 50, 40, 40, 0, 0], // 扣完变负 → 归零，不返回负数
  ];
  for (const [w, h, px, py, ew, eh] of cases) {
    const r = contentArea(w, h, px, py);
    if (r.availW !== ew || r.availH !== eh) {
      fail(`contentArea(${w},${h},${px},${py}) 期望 ${ew}x${eh}，实得 ${r.availW}x${r.availH}`);
    }
  }
  console.log(`   ${cases.length} 例通过`);
}

/* ═══ 5. 端到端：改窗口大小也不会溢出（含极小窗口）════════ */

console.log('5. 端到端：任意窗口尺寸下都不溢出');
{
  let bad = 0, n = 0;
  for (let vw = 320; vw <= 3840; vw += 137) {
    for (let vh = 320; vh <= 2160; vh += 113) {
      for (const [iw, ih] of [[1920, 1080], [3840, 2160], [800, 600]]) {
        n++;
        const box = contentArea(vw, vh - TOOLBAR_H, AREA_PAD, AREA_PAD);
        const { w, h } = fitDisplaySize({
          availW: box.availW,
          availH: box.availH,
          imgW: iw,
          imgH: ih,
        });
        if (w - box.availW > 0.5 || h - box.availH > 0.5) {
          bad++;
          if (bad <= 3) fail(`视口 ${vw}x${vh} + 图 ${iw}x${ih} 溢出`);
        }
      }
    }
  }
  console.log(`   ${n} 例：违反 ${bad} 例`);
  if (bad > 0) fail(`共 ${bad} 例溢出`);
}

/* ═══ 6. 框选弹窗 CropOverlay：旧硬编码反而溢出 ═════════════ */

// CropOverlay 用 inline styles，留白只能按字号估算（**仅用于量化，实现不依赖**）：
//   header  padding 12+12 + border 1 + 标题 14px*1.2 + gap 4 + hint 12px*1.2 ≈ 60
//   footer  padding 10+10 + border 1 + 按钮(12px*1.2 + 8)                  ≈ 43
//   canvasWrap padding 12*2                                               = 24
//   panel   maxHeight 94vh
const CROP_CHROME = 60 + 43 + 24;

const cropAvail = (vw, vh) => ({ w: vw, h: vh * 0.94 - CROP_CHROME });

console.log('6. 框选弹窗：旧硬编码（innerHeight-150）vs 实测可用区');
{
  let oldBad = 0, newBad = 0, n = 0, oldMax = 0;
  let bigger = 0, smaller = 0;
  const samples = [];
  for (const [vw, vh] of VIEWPORTS) {
    for (const [iw, ih] of SHOTS) {
      const av = cropAvail(vw, vh);
      if (!(av.h > 0)) continue;
      n++;
      const so = Math.min((vw * 0.92) / iw, (vh - 150) / ih, 1);
      const over = ih * so - av.h;
      if (over > 0.5) {
        oldBad++;
        if (over > oldMax) oldMax = over;
        if (samples.length < 4) {
          samples.push(
            `     屏幕 ${vw}x${vh} + 截图 ${iw}x${ih}：scale=${so.toFixed(3)} → ` +
              `canvas 高 ${(ih * so).toFixed(0)}，可用 ${av.h.toFixed(0)} → ` +
              `溢出 ${over.toFixed(0)}px（canvasWrap 出滚动条）`
          );
        }
      }
      const sn = containScale({ availW: av.w, availH: av.h, imgW: iw, imgH: ih });
      if (ih * sn - av.h > 0.5 || iw * sn - av.w > 0.5) {
        newBad++;
        fail(`新实现仍溢出 ${vw}x${vh} + ${iw}x${ih}`);
      }
      if (sn > so + 1e-9) bigger++;
      else if (sn < so - 1e-9) smaller++;
    }
  }
  console.log(`   ${n} 种组合（可用高按 ${CROP_CHROME}px 常量估算）`);
  console.log(`   旧实现：溢出 ${oldBad}/${n} 例（最大超 ${oldMax.toFixed(0)}px → 滚动条）`);
  console.log(`   新实现：溢出 ${newBad}/${n} 例`);
  for (const s of samples) console.log(s);
  console.log(`   显示尺寸：新实现更大 ${bigger} 例 / 更小 ${smaller} 例`);
  if (newBad > 0) fail(`新实现仍有 ${newBad} 例溢出`);
  if (bigger === 0) fail('新实现没有把图显示得更大，说明量化或实现有问题');
}

/* ═══ 7. 主预览容器：遗留的 16:9 高度锁 ════════════════════ */

const P = await import(new URL('../src/lib/previewBox.ts', import.meta.url).href);
const { previewBox, PREVIEW_MAX_HEIGHT } = P;

/**
 * 模拟 `.step-detail-image` 的最终盒子，并算出 contain 留白。
 *   boxW = min(父容器宽, maxWidth)          maxWidth 由 previewBox 给出
 *   高度：
 *     现状（遗留 useLayoutEffect 写 inline height）→ min(max(boxW*9/16,200),520)，
 *                                                    再被 CSS max-height:420 截断
 *     修后（删掉 inline height）              → boxW/aspect（aspect-ratio 生效，恒 ≤420）
 */
function previewLetterbox(parentW, imgW, imgH, locked) {
  const box = previewBox(imgW, imgH);
  if (!box) return null;
  const boxW = Math.min(parentW, box.maxWidth);
  const boxH = locked
    ? Math.min(Math.min(Math.max((boxW * 9) / 16, 200), 520), PREVIEW_MAX_HEIGHT)
    : boxW / box.aspect;
  const s = Math.min(boxW / imgW, boxH / imgH);
  return {
    boxW,
    boxH,
    gapX: boxW - imgW * s,
    gapY: boxH - imgH * s,
  };
}

console.log('7. 主预览容器：遗留的 16:9 高度锁（v24 只修了宽度，高度仍被锁死）');
{
  // 真实会遇到的：整屏截图 + 裁图后的极端宽高比（type 步骤裁出来可到 7:1）
  const IMGS = [
    [1920, 1080, '整屏 16:9'],
    [1366, 768, '整屏 16:9 小'],
    [1440, 900, '整屏 16:10'],
    [1512, 982, 'MacBook'],
    [3440, 1440, '带鱼屏 21:9'],
    [1024, 768, '4:3'],
    [448, 280, 'click 裁图'],
    [416, 56, 'type 裁图 7.4:1'],
    [316, 56, 'click 裁图 5.6:1'],
    [48, 56, '超小裁图 0.86:1'],
  ];
  const PARENTS = [600, 800, 1000, 1200];

  let oldBad = 0, newBad = 0, n = 0, oldMaxX = 0, oldMaxY = 0;
  const samples = [];
  for (const [iw, ih, label] of IMGS) {
    for (const pw of PARENTS) {
      n++;
      const o = previewLetterbox(pw, iw, ih, true);
      const f = previewLetterbox(pw, iw, ih, false);
      if (!o || !f) continue;
      if (o.gapX > 0.5 || o.gapY > 0.5) {
        oldBad++;
        oldMaxX = Math.max(oldMaxX, o.gapX);
        oldMaxY = Math.max(oldMaxY, o.gapY);
        if (samples.length < 6) {
          samples.push(
            `     ${label} ${iw}x${ih} @ 父宽${pw}：盒 ${o.boxW.toFixed(0)}x${o.boxH.toFixed(0)}` +
              ` → 左右各 ${(o.gapX / 2).toFixed(0)}px / 上下各 ${(o.gapY / 2).toFixed(0)}px 灰边`
          );
        }
      }
      if (f.gapX > 0.5 || f.gapY > 0.5) {
        newBad++;
        fail(`修后仍有留白 ${label} ${iw}x${ih} @ 父宽${pw}：${f.gapX.toFixed(1)}/${f.gapY.toFixed(1)}`);
      }
    }
  }
  console.log(`   ${n} 种组合（${IMGS.length} 种图 × ${PARENTS.length} 种父宽）`);
  console.log(`   现状：留白 ${oldBad}/${n} 例（左右最大 ${oldMaxX.toFixed(0)}px / 上下最大 ${oldMaxY.toFixed(0)}px）`);
  console.log(`   修后：留白 ${newBad}/${n} 例`);
  for (const s of samples) console.log(s);
  if (newBad > 0) fail(`修后仍有 ${newBad} 例留白`);

  // 静态防线：inline height 会盖掉 CSS aspect-ratio，不许再给主预览写死高度
  const { readFileSync } = await import('node:fs');
  const se = readFileSync(new URL('../src/pages/StepEditor.tsx', import.meta.url), 'utf8');
  const seCode = se.replace(/\/\*[\s\S]*?\*\//g, '').replace(/\/\/.*$/gm, '');
  for (const re of [/\.style\.height\s*=/, /w \* 9 \/ 16|9 \/ 16|0\.5625/]) {
    if (re.test(seCode)) {
      fail(`StepEditor 写死高度（命中 ${re}）—— inline height 会盖掉 aspect-ratio，重新引入 letterbox`);
    }
  }
  console.log('   静态防线：主预览容器不得写死高度 ✓');
}

/* ═══ 8. 命中容差：图像像素 vs 屏幕像素 ═════════════════════ */

const H = await import(new URL('../src/lib/annoHit.ts', import.meta.url).href);
const { HIT_SLOP, slopPx, hitAnnotation, hitCropHandle } = H;

console.log('8. 命中容差：阈值必须按屏幕像素定义（除以 scale 后才能用）');
{
  // 标注弹窗的可用区 = 窗口 -(padding 40*2) -(工具栏 56 + padding 40*2)
  const VIEWPORTS2 = [
    [1366, 768],
    [1600, 900],
    [1920, 1080],
    [2560, 1440],
  ];
  const SHOTS2 = [
    [1366, 768, '1x 笔记本'],
    [1920, 1080, '1080p'],
    [2560, 1440, '2K'],
    [3440, 1440, '带鱼屏'],
    [3840, 2160, '4K'],
  ];

  // 阈值写死成图像像素时，屏幕上的实际容差 = 阈值 × scale
  const rows = [];
  let worst = Infinity;
  let tooSmall = 0;
  let n = 0;
  for (const [vw, vh] of VIEWPORTS2) {
    for (const [iw, ih, label] of SHOTS2) {
      const box = contentArea(vw, vh - TOOLBAR_H, AREA_PAD, AREA_PAD);
      const s = containScale({ availW: box.availW, availH: box.availH, imgW: iw, imgH: ih });
      n++;
      const screenSlop = HIT_SLOP.arrow * s; // 旧实现：12 被当成图像像素
      if (screenSlop < worst) worst = screenSlop;
      if (screenSlop < 6) tooSmall++; // 屏幕 <6px 基本点不中
      if (rows.length < 5 && s < 0.6) {
        rows.push(
          `     ${label} ${iw}x${ih} @ 窗口 ${vw}x${vh}：scale=${s.toFixed(3)}` +
            ` → 箭头容差 屏幕 ${screenSlop.toFixed(1)}px（应为 ${HIT_SLOP.arrow}px）`
        );
      }
    }
  }
  console.log(`   ${n} 种组合（${SHOTS2.length} 种截图 × ${VIEWPORTS2.length} 种窗口）`);
  console.log(`   旧实现：箭头容差在屏幕上最小只有 ${worst.toFixed(1)}px（目标是 ${HIT_SLOP.arrow}px）`);
  console.log(`   旧实现：屏幕容差 <6px（基本点不中）的有 ${tooSmall}/${n} 例`);
  for (const r of rows) console.log(r);

  // 新实现：容差恒定 = 屏幕像素定义值（无论如何缩放）
  let bad = 0;
  for (const [vw, vh] of VIEWPORTS2) {
    for (const [iw, ih] of SHOTS2) {
      const box = contentArea(vw, vh - TOOLBAR_H, AREA_PAD, AREA_PAD);
      const s = containScale({ availW: box.availW, availH: box.availH, imgW: iw, imgH: ih });
      const m = slopPx(HIT_SLOP.arrow, s);
      if (Math.abs(m * s - HIT_SLOP.arrow) > 0.01) {
        bad++;
        fail(`scale=${s} 时 屏幕容差 ${(m * s).toFixed(2)} != ${HIT_SLOP.arrow}`);
      }
    }
  }
  console.log(`   新实现：屏幕容差恒为 ${HIT_SLOP.arrow}px（${n} 例，违反 ${bad} 例）`);
  if (bad > 0) fail(`命中容差换算 ${bad} 例错误`);

  // slopPx 边界
  for (const s of [0, -1, NaN, Infinity]) {
    if (slopPx(8, s) !== 8) fail(`slopPx(8, ${s}) 应回落 8`);
  }
  if (slopPx(0, 0.5) !== 0) fail('slopPx(0) 应为 0');
  if (!(slopPx(8, 1000) >= 1)) fail('极端缩小时容差不能塌到 0');

  // 命中函数本身：容差内必须命中、容差外必须不命中
  const box = { type: 'highlight', x: 100, y: 100, width: 80, height: 40 };
  if (!hitAnnotation(box, 100, 100, 8)) fail('包围盒左上角应命中');
  if (!hitAnnotation(box, 95, 96, 8)) fail('外扩 8px 内应命中');
  if (hitAnnotation(box, 90, 100, 8)) fail('外扩 8px 外不应命中');
  const arrow = { type: 'arrow', x: 0, y: 0, endX: 100, endY: 0 };
  if (!hitAnnotation(arrow, 50, 5, 12)) fail('箭头线上方 5px 应命中');
  if (hitAnnotation(arrow, 50, 20, 12)) fail('箭头线上方 20px 不应命中');
  if (hitAnnotation(arrow, -50, 0, 12)) fail('箭头线段延长线外不应命中（t 必须 clamp）');

  // 裁剪框：四角优先于四边；框内为 move
  const cr = { x: 0, y: 0, width: 100, height: 100 };
  if (hitCropHandle(cr, 2, 2, 14, 16) !== 'nw') fail('左上角应命中 nw');
  if (hitCropHandle(cr, 50, 2, 14, 16) !== 'n') fail('上边中点应命中 n');
  if (hitCropHandle(cr, 50, 50, 14, 16) !== 'move') fail('框内应命中 move');
  if (hitCropHandle(cr, 200, 200, 14, 16) !== null) fail('框外不应命中');
  console.log('   命中函数单元断言：通过');
}

/* ═══ 9. 静态防线：两个弹窗都必须走实测可用区 ═══════════════ */

console.log('9. 静态防线：组件源码不得再出现「window 尺寸直接当可用区」');
{
  const { readFileSync } = await import('node:fs');
  const readSrc = (p) => readFileSync(new URL(p, import.meta.url), 'utf8');

  const spec = [
    {
      file: '../src/components/AnnotationsOverlay.tsx',
      name: '标注弹窗',
      mustHave: [
        ['ResizeObserver', '必须实测容器内容区'],
        ['containScale(', '必须调用等比 contain'],
        ['contentArea(', '窗口兜底必须扣掉 padding'],
        ['ref={areaRef}', '内容区容器必须挂 ref'],
      ],
      forbidden: [/window\.innerHeight\s*-\s*56/, /window\.innerWidth\s*\*\s*0\.995/],
      // 按**函数体**断言：整个函数里必须出现容差换算调用。
      // 早先的写法是 forbidden 正则匹配 `const m = 8;`，结果被 `? 12 : 8`
      // 这种三元表达式绕过去了 —— 反向验证时才发现。改成摘函数体更稳。
      bodies: [
        ['const findAt = ', /annoSlop\(|slopPx\(/, 'findAt 必须用 annoSlop 换算容差'],
        ['const hitTestCrop = ', /cropSlop\(|slopPx\(/, 'hitTestCrop 必须用 cropSlop 换算容差'],
      ],
    },
    {
      file: '../src/components/CropOverlay.tsx',
      name: '框选弹窗',
      mustHave: [
        ['ResizeObserver', '必须实测容器内容区'],
        ['containScale(', '必须调用等比 contain'],
        ['ref={wrapRef}', '内容区容器必须挂 ref'],
        ["useState<'loading' | 'ready' | 'error'>", '加载中不能误报「加载失败」'],
      ],
      forbidden: [/window\.innerHeight\s*-\s*150/, /window\.innerWidth\s*\*\s*0\.92/],
    },
  ];

  for (const s of spec) {
    const src = readSrc(s.file);
    for (const [needle, why] of s.mustHave) {
      if (!src.includes(needle)) fail(`${s.name} 源码缺少 ${needle}（${why}）`);
    }
    // ⚠️ 只扫代码，先剥掉注释——历史 bug 的说明文字里会提到旧写法，否则误报
    const code = src.replace(/\/\*[\s\S]*?\*\//g, '').replace(/\/\/.*$/gm, '');
    for (const re of s.forbidden) {
      if (re.test(code)) fail(`${s.name} 源码出现禁用的硬编码 ${re}（应改用实测内容区）`);
    }
    // 摘出单个函数体做断言（从 `const fn = ` 到下一个顶层 `\n  };`）
    for (const [head, must, why] of s.bodies ?? []) {
      const i = code.indexOf(head);
      if (i < 0) {
        fail(`${s.name} 找不到函数 ${head}`);
        continue;
      }
      const end = code.indexOf('\n  };', i);
      const body = end > 0 ? code.slice(i, end) : code.slice(i, i + 600);
      if (!must.test(body)) fail(`${s.name} 的 ${head.trim()} —— ${why}`);
    }
  }

  // 布局常量必须与 CSS 对齐（改 CSS 忘了改常量，兜底值就会错）
  const css = readSrc('../src/styles.css');
  const cssHas = (cls, prop, val) =>
    new RegExp(`\\.${cls}\\s*\\{[^}]*${prop}:\\s*${val}`).test(css);
  if (!cssHas('annotation-toolbar', 'height', '56px')) {
    fail('styles.css 的 .annotation-toolbar 高度不是 56px，组件常量 TOOLBAR_H 已失配');
  }
  if (!cssHas('annotation-canvas-area', 'padding', '40px')) {
    fail('styles.css 的 .annotation-canvas-area padding 不是 40px，组件常量 CANVAS_AREA_PAD 已失配');
  }
  console.log('   通过（两个弹窗都实测容器 + 常量与 CSS 对齐）');
}

/* ═══ 10. 标注视觉权重：必须与截图分辨率无关 ═══════════════ */
/**
 * 不变式：**两张图在屏幕上显示为同样高度时，标注的粗细/字号必须看起来一样大。**
 *
 * 现状 `src/lib/annotate.ts` 把 strokeWidth / fontSize / 箭头头部长度写死成
 * **图像像素**（`a.strokeWidth || 3`、`a.fontSize || 24`、`14 + sw*2`），
 * 而 canvas 是按图像分辨率画的、再用 CSS 缩到 `scale` 显示
 * （`canvas.width = image.width` + `style.width = image.width * scale`）。
 * 于是屏幕上的实际观感 = 图像像素值 × scale，而 scale 又随截图分辨率变化 ——
 * 4K 截图缩到 0.29 显示时，24px 的字只有 7px，3px 的箭头线只有 0.9px。
 *
 * 与第 8 段「命中容差」是同一类错误：**把某一坐标系的数字用到了另一坐标系**，
 * 只是方向相反（那边该除以 scale，这边该乘以图高比例）。
 */
console.log('10. 标注视觉权重：必须与截图原始分辨率无关');
{
  // annotate.ts 里写死的**图像像素**默认值
  const STYLE = [
    ['高亮边框', 2],
    ['箭头线宽', 3],
    ['箭头头部', 14 + 3 * 2],
    ['模糊虚线', 4],
    ['文字字号', 24],
  ];
  const REF_H = 1080; // 归一化基准：1080p 观感完全不变，只缩放其它分辨率

  const WINS = [[1366, 768], [1920, 1080], [2560, 1440]];

  // ── (a) 相对权重 = 屏幕观感 / 图片显示高度，理想情况下与分辨率无关 ──
  const agg = new Map(); // name -> {min, max}
  for (const [vw, vh] of WINS) {
    const box = contentArea(vw, vh - TOOLBAR_H, AREA_PAD, AREA_PAD);
    for (const [iw, ih] of SHOTS) {
      const s = containScale({ availW: box.availW, availH: box.availH, imgW: iw, imgH: ih });
      const dispH = ih * s; // 图片在屏幕上的显示高度
      for (const [name, px] of STYLE) {
        const oldW = (px * s) / dispH;              // 现状
        const newW = (px * (ih / REF_H) * s) / dispH; // 按图高等比
        const a = agg.get(name) ?? { oldMin: Infinity, oldMax: -Infinity, newMin: Infinity, newMax: -Infinity };
        a.oldMin = Math.min(a.oldMin, oldW); a.oldMax = Math.max(a.oldMax, oldW);
        a.newMin = Math.min(a.newMin, newW); a.newMax = Math.max(a.newMax, newW);
        agg.set(name, a);
      }
    }
  }

  console.log('   (a) 相对权重（屏幕观感 ÷ 图片显示高度），跨 8 种截图分辨率：');
  let worst = 0;
  for (const [name] of STYLE) {
    const a = agg.get(name);
    const ratio = a.oldMax / a.oldMin;
    const nRatio = a.newMax / a.newMin;
    worst = Math.max(worst, ratio);
    console.log(
      `     ${name}：现状极差 ${ratio.toFixed(2)}×` +
      `（${(a.oldMin * 100).toFixed(3)}% ~ ${(a.oldMax * 100).toFixed(3)}% 图高）` +
      ` → 归一化后 ${nRatio.toFixed(2)}×`
    );
  }
  console.log(`     最大不一致 ${worst.toFixed(2)}× —— 同一张图放大分辨率，标注就变细 ${worst.toFixed(1)} 倍`);

  // ── (b) 具体观感：固定窗口 1366×768，各分辨率截图下的字号/线宽 ──
  const [vw, vh] = [1366, 768];
  const box = contentArea(vw, vh - TOOLBAR_H, AREA_PAD, AREA_PAD);
  console.log(`   (b) 编辑器里实际看到的尺寸（窗口 ${vw}×${vh}，可用 ${box.availW}×${box.availH}）：`);
  console.log('     注：宽度受限的图（如带鱼屏）整体显示得小，标注随之变小是**正确**的；');
  console.log('     判据是「相对权重是否恒定」，不是「绝对像素是否相同」。');
  for (const [iw, ih] of SHOTS) {
    const s = containScale({ availW: box.availW, availH: box.availH, imgW: iw, imgH: ih });
    const dispH = ih * s;
    const bound = dispH < box.availH - 0.5 ? '宽受限' : '高受限';
    const fontOld = 24 * s;
    const fontNew = 24 * (ih / REF_H) * s;
    const lineOld = 3 * s;
    const lineNew = 3 * (ih / REF_H) * s;
    console.log(
      `     ${String(iw + '×' + ih).padEnd(10)} scale=${s.toFixed(3)} 显示高 ${dispH.toFixed(0).padStart(3)}px ${bound} → ` +
      `字号 ${fontOld.toFixed(1).padStart(4)}px / 箭头线宽 ${lineOld.toFixed(2)}px` +
      `　归一化后 ${fontNew.toFixed(1).padStart(4)}px / ${lineNew.toFixed(2)}px`
    );
  }
  console.log('     ⚠️ 高受限的几行显示高度都是 632px，但字号从 21.1px 一路掉到 7.0px —— 这就是观感不一致');
  console.log('     ℹ️ 本段为度量段（暂不阻断）；确认方案后改为硬断言 + 静态防线');
}

/* ═══ 结论 ════════════════════════════════════════════════ */

console.log('');
if (failures) {
  console.log(`❌ 共 ${failures} 项失败`);
  process.exit(1);
}
console.log('✅ 缩放适配（标注弹窗 + 框选弹窗）全部通过');
