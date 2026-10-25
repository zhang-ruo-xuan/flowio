/**
 * T9「发布到团队库」的口径单点（纯函数，可单测）。
 *
 * ⚠️ 本文件与**浏览器扩展仓** `src/shared/teamPublish.ts` 是**同一份口径的两份落地**
 *    （两个仓库互相 import 不到）。改文案/改判据时**两边都要改**，措辞必须一致 ——
 *    否则同一个动作在桌面端说「已发布到网站，但没能放进团队库」、在扩展里说「发布失败」，
 *    用户会以为其中一个坏了。
 *
 * 这条链上有三处**都不会报错、只会说错话/给错选项**的地方，都在这里实现一次：
 *   ① 哪些空间该出现在下拉里（过滤口径）
 *   ② 附挂失败时的措辞（**不能把「发布成功 + 附挂失败」说成「发布失败」**）
 *   ③ 没选空间时的收尾语（不该凭空冒出「团队库」字样）
 *
 * 🔴 过滤口径**只用服务端给的 `can_publish`**，不在这里重算角色 ——
 *    见 `GET /api/ws`（口径单点在网站的 `lib/workspace.ts::canPublishToWorkspace`）。
 */

/** 服务端 `GET /api/ws` 的 `items[i]` 里我们关心的字段（其余忽略）。 */
export interface TeamWorkspaceRaw {
  id?: unknown;
  name?: unknown;
  role?: unknown;
  planLabel?: unknown;
  member_status?: unknown;
  can_publish?: unknown;
}

export interface TeamWorkspace {
  id: string;
  name: string;
  role: string;
  planLabel: string;
}

/** 「不发布到团队库」的选项值（空串 ⇒ 不附挂）。 */
export const TEAM_PUBLISH_NONE = "";
export const TEAM_PUBLISH_LABEL = "发布到团队库（可选）";
export const TEAM_PUBLISH_NONE_LABEL = "不发布到团队库";
/** 一个可发布空间都没有时的说明（不是错误，只是没得选）。 */
export const TEAM_PUBLISH_EMPTY_HINT =
  "没有可发布的团队空间 —— 需要先加入（或创建一个）团队，并拥有创作者及以上角色";

/**
 * 从 `/api/ws` 的原始 items 里挑出**可以发教程**的空间。
 *
 * 🔴 只认服务端的 `can_publish === true`；字段缺失 / 其它值一律**不出现**（fail-closed）。
 */
export function pickPublishableWorkspaces(items: unknown): TeamWorkspace[] {
  const arr = Array.isArray(items) ? items : [];
  const out: TeamWorkspace[] = [];
  const seen = new Set<string>();
  for (const it of arr) {
    const r = (it || {}) as TeamWorkspaceRaw;
    if (r.can_publish !== true) continue;
    const id = String(r.id ?? "").trim();
    if (!id || seen.has(id)) continue;
    seen.add(id);
    out.push({
      id,
      name: String(r.name ?? "").trim() || "未命名空间",
      role: String(r.role ?? "").trim(),
      planLabel: String(r.planLabel ?? "").trim(),
    });
  }
  return out;
}

/** 下拉里显示的名字（带套餐，方便同名空间区分）。 */
export function teamWorkspaceLabel(ws: TeamWorkspace): string {
  const bits = [ws.name];
  if (ws.planLabel) bits.push(ws.planLabel);
  return bits.join(" · ");
}

export function teamWorkspaceName(list: readonly TeamWorkspace[], id: string): string {
  const hit = list.find((w) => w.id === id);
  return hit ? hit.name : "";
}

/** 附挂成功。 */
export function teamAttachOkText(wsName: string): string {
  return `已发布到网站，并放进「${wsName || "团队库"}」`;
}

/**
 * 附挂失败。
 * 🔴 措辞是本文件存在的主要理由：**绝不能把这句话说成「发布失败」** ——
 *    发布本身成功了，用户手上已经有链接；团队库只是少了一步，还能在网站手工补。
 *    说成「发布失败」会让人重发一遍，制造重复教程。
 */
export function teamAttachFailText(wsName: string, err?: string): string {
  const detail = String(err ?? "").trim();
  return (
    `教程已发布到网站，但没能放进「${wsName || "团队库"}」` +
    (detail ? `：${detail}` : "") +
    "（可在网站的团队库页手动添加）"
  );
}

/** 没选空间 / 没有空间可选时的收尾语（保持原样，不提团队库）。 */
export function composePublishToast(opts: {
  base: string;
  target: string;
  list: readonly TeamWorkspace[];
  attach?: { ok: boolean; error?: string } | null;
}): string {
  const target = String(opts.target ?? "").trim();
  if (!target) return opts.base;
  const a = opts.attach;
  if (!a) return opts.base;
  return a.ok
    ? `${opts.base}；${teamAttachOkText(teamWorkspaceName(opts.list, target))}`
    : `${opts.base}；⚠️ ${teamAttachFailText(teamWorkspaceName(opts.list, target), a.error)}`;
}
