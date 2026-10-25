/**
 * 团队库的网络层（T9「发布到团队库」）。
 *
 * 🔴 两个函数都**永不抛**：
 *   · `listMyWorkspaces` 失败 ⇒ `[]`（发布到网站是主线，团队库是附加，取不到列表不该挡发布）
 *   · `attachTutorialToWorkspace` 失败 ⇒ `{ ok:false, error }`，由调用方按
 *     「已发布，但没能放进团队库」的措辞呈现（见 `lib/teamPublish.ts`）
 *
 * ⚠️ 与扩展仓 `src/lib/site-api.ts` 里的同名函数是**同一份契约的两份落地**（两仓互相
 *    import 不到）：都是 `GET /api/ws` + `POST /api/ws/<id>/tutorials`，都带上裸 token
 *    （网站的 `authUserPB` 会剥掉可选的 `Bearer ` 前缀）。改接口时两边都要改。
 *
 * token 由调用方显式传入（桌面端把它放在 React state 里，不在 localStorage）。
 */

import {
  pickPublishableWorkspaces,
  type TeamWorkspace,
} from "@/lib/teamPublish";

const SITE = "https://flowio.cn";

/** 我所在的、**可以发教程**的团队空间（没有 / 取不到 ⇒ 空数组）。 */
export async function listMyWorkspaces(token: string | null | undefined): Promise<TeamWorkspace[]> {
  const t = String(token || "").trim();
  if (!t) return [];
  try {
    const resp = await fetch(`${SITE}/api/ws`, { headers: { Authorization: t } });
    if (!resp.ok) return [];
    const data = await resp.json();
    return pickPublishableWorkspaces((data || {}).items);
  } catch {
    return [];
  }
}

/** 把已发布的教程挂进团队库。**永不抛** —— 返回 `{ ok, error }`。 */
export async function attachTutorialToWorkspace(
  token: string | null | undefined,
  workspaceId: string,
  guideId: string,
): Promise<{ ok: boolean; error?: string }> {
  const t = String(token || "").trim();
  const ws = String(workspaceId || "").trim();
  const gid = String(guideId || "").trim();
  if (!t) return { ok: false, error: "未登录网站账号" };
  if (!ws || !gid) return { ok: false, error: "缺少空间或教程标识" };
  try {
    const resp = await fetch(`${SITE}/api/ws/${encodeURIComponent(ws)}/tutorials`, {
      method: "POST",
      headers: { "Content-Type": "application/json", Authorization: t },
      body: JSON.stringify({ guideId: gid }),
    });
    if (resp.ok) return { ok: true };
    let msg = `HTTP ${resp.status}`;
    try {
      const d = await resp.json();
      if (d && typeof d.error === "string" && d.error) msg = d.error;
    } catch { /* 非 JSON 响应，保留状态码 */ }
    return { ok: false, error: msg };
  } catch (e) {
    return { ok: false, error: e instanceof Error ? e.message : String(e) };
  }
}
