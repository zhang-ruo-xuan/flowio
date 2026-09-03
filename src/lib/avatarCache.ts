/**
 * 网站同步头像的本地缓存（内存 + localStorage 双层）。
 *
 * 问题背景：顶栏/个人中心/设置页的头像 <img> 直连 flowio.cn 的 PB 文件 API，
 * 该响应无 Cache-Control 头 → WebView2 不缓存 → 每次组件重挂（如退出教程
 * 编辑器）都重新下载，下载期间露出容器的绿色底 → 头像「绿→网站头像」跳变。
 *
 * 方案：img 首次加载成功后用 fetch 把图转成 dataURL 存双层缓存；
 * 之后渲染时同步命中缓存，零网络请求、零中间态、离线也可用。
 * 头像一般几十 KB，localStorage 单键存一个绰绰有余；写失败静默忽略。
 */

const MEM = new Map<string, string>();
const LS_PREFIX = "flowio-avatar:";
const PB_BASE = "https://flowio.cn";

export function avatarFileUrl(id: string, avatar: string): string {
  return `${PB_BASE}/api/files/users/${id}/${avatar}`;
}

function avatarKey(id: string, avatar: string): string {
  return `${id}/${avatar}`;
}

/** 同步取缓存的 dataURL；未命中返回 null（调用方回退到网络 URL）。 */
export function getCachedAvatar(id: string, avatar: string): string | null {
  if (!id || !avatar) return null;
  const k = avatarKey(id, avatar);
  const mem = MEM.get(k);
  if (mem) return mem;
  try {
    const ls = localStorage.getItem(LS_PREFIX + k);
    if (ls) {
      MEM.set(k, ls);
      return ls;
    }
  } catch {
    /* localStorage 不可用（如隐私模式），仅内存层降级 */
  }
  return null;
}

/** 异步下载头像并写入双层缓存。已缓存（内存层）则跳过；失败静默，下次再试。 */
export function cacheAvatar(id: string, avatar: string): void {
  if (!id || !avatar) return;
  const k = avatarKey(id, avatar);
  if (MEM.has(k)) return;
  fetch(avatarFileUrl(id, avatar))
    .then((r) => {
      if (!r.ok) throw new Error(`avatar fetch HTTP ${r.status}`);
      return r.blob();
    })
    .then(
      (b) =>
        new Promise<string>((res, rej) => {
          const fr = new FileReader();
          fr.onload = () => res(String(fr.result));
          fr.onerror = () => rej(fr.error);
          fr.readAsDataURL(b);
        })
    )
    .then((dataUrl) => {
      MEM.set(k, dataUrl);
      try {
        localStorage.setItem(LS_PREFIX + k, dataUrl);
      } catch {
        /* 配额超限等：仅保留内存层 */
      }
    })
    .catch(() => {
      /* 静默失败：网络抖动/离线时下次 img onLoad 再触发 */
    });
}
