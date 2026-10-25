/**
 * r65「在网页编辑」的口径单点。
 *
 * ⚠️ 与扩展仓 `src/shared/webEdit.ts` 是**同一份口径的两次落地**（跨仓库无法共享代码，
 *    与 `teamPublish.ts` 在三端各留一份是同一个约定）。**改文案或 URL 形态时两边都要改。**
 *
 * 背景（方案 §6 第 20 条）：用户拍板「先对齐 Scribe」——
 * 录制端只采集，编辑 / 发布 / 分享收敛到网页。Scribe 是 Stop 之后自动开网页编辑器；
 * Flowio 的端内编辑器要留给离线版 / 私有化，所以这一轮先做**显式入口**。
 *
 * 🔴 两条纪律：
 *   ① 桌面端走中间层 `/api/guides/upload`，**编辑地址由服务端算好回传**（`guide.edit_url`）——
 *      这里只保留「本地记了 slug、不想再传一次」时的兜底拼装；
 *   ② 打不开**绝不许说成「发布失败」**：端内内容一切照旧，用户什么都没丢。
 */

export const SITE_ORIGIN = "https://flowio.cn";

/** ⚠️ 必须与网站 `app/studio/edit/[slug]/page.tsx` 的路由一致。 */
export const WEB_EDIT_PATH = "/studio/edit/";

/** 本地记住的「录制 → 云端 slug」映射键。⚠️ 与扩展仓 `flowio_webslug_<rid>` 同名。 */
export const webSlugKey = (recordingId: string) => `flowio_webslug_${recordingId}`;

/** 兜底拼装：slug 为空 ⇒ 空串（绝不拼出半截 URL 让用户撞 404）。 */
export function webEditUrl(slug: string, origin: string = SITE_ORIGIN): string {
  const s = (slug || "").trim();
  if (!s) return "";
  return `${origin}${WEB_EDIT_PATH}${encodeURIComponent(s)}`;
}

export const WEB_EDIT_BUTTON = "在网页编辑";
export const WEB_EDIT_HINT = "到 flowio.cn 排版、发团队库、出分享链接";
export const WEB_EDIT_NEED_LOGIN = "请先登录 flowio.cn，再打开网页编辑";
export const WEB_EDIT_OPEN_FAILED = "暂时打不开网页编辑，你在这里的编辑不受影响";
