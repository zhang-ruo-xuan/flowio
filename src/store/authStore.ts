import { create } from 'zustand';
import { persist } from 'zustand/middleware';

interface SiteUser {
  id: string;
  email?: string;
  display_name?: string;
  bio?: string;
  avatar?: string;
  token: string; // 每个账号各自保存 token，便于独立刷新 / 退出
}

// setAuth 调用方只传基础信息，token 由第一个参数提供
type SiteUserInput = Omit<SiteUser, 'token'>;

interface AuthState {
  siteUser: SiteUser | null; // 当前登录账号
  siteToken: string | null; // 当前登录 token
  setAuth: (token: string, user: SiteUserInput) => void;
  clearAuth: () => void;
  isLoggedIn: () => boolean;
}

// 网站集成：PocketBase 认证状态（来自 flowio.cn 网站登录）
// 使用 zustand persist 持久化到 localStorage，桌面端重启后保留登录态
// 单账号模式：同一时间只保留一个登录账号；再次登录直接覆盖旧账号，退出即清空
export const useAuthStore = create<AuthState>()(
  persist(
    (set, get) => ({
      siteUser: null,
      siteToken: null,

      setAuth: (token, user) =>
        set({ siteUser: { ...user, token }, siteToken: token }),

      clearAuth: () => set({ siteUser: null, siteToken: null }),
      isLoggedIn: () => !!get().siteToken,
    }),
    {
      name: 'flowio-auth-storage', // localStorage key
      version: 2,
      // 兼容旧版存储结构：
      // v0: { siteToken, siteUser } 单账号
      // v1: { siteUsers: [...], activeId } 多账号 —— 取当前激活（或最后一个）账号迁移为单账号
      migrate: (persisted: any, version: number): any => {
        if (!persisted) return { siteUser: null, siteToken: null };
        if (version < 2) {
          if (Array.isArray(persisted.siteUsers) && persisted.siteUsers.length > 0) {
            const users = persisted.siteUsers;
            const active = users.find((u: any) => u.id === persisted.activeId) ?? users[users.length - 1];
            return { siteUser: active ?? null, siteToken: active?.token ?? null };
          }
          if (persisted.siteUser) {
            return { siteUser: { ...persisted.siteUser, token: persisted.siteToken ?? '' }, siteToken: persisted.siteToken ?? null };
          }
          return { siteUser: null, siteToken: null };
        }
        return persisted;
      },
    }
  )
);
