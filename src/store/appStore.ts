import { create } from 'zustand';

// ============================================================
// App global state
// ============================================================

export interface AppState {
  darkMode: boolean;
  activeRoute: string;
  isRecording: boolean;
  recordingId: string | null;
  // 正在后台执行 AI 精简的教程 id（跨页面保留，用于「AI 精简中…」指示）
  simplifyingId: string | null;

  toggleDarkMode: () => void;
  setActiveRoute: (route: string) => void;
  setRecording: (recording: boolean, recordingId?: string | null) => void;
  setSimplifyingId: (id: string | null) => void;
}

export const useAppStore = create<AppState>((set) => ({
  darkMode: false,
  activeRoute: '/',
  isRecording: false,
  recordingId: null,
  simplifyingId: null,

  toggleDarkMode: () => set((s) => ({ darkMode: !s.darkMode })),
  setActiveRoute: (route) => set({ activeRoute: route }),
  setRecording: (recording, recordingId = null) =>
    set({ isRecording: recording, recordingId }),
  setSimplifyingId: (id) => set({ simplifyingId: id }),
}));
