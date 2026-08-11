import { create } from 'zustand';

// ============================================================
// App global state
// ============================================================

export interface AppState {
  darkMode: boolean;
  activeRoute: string;
  isRecording: boolean;
  recordingId: string | null;

  toggleDarkMode: () => void;
  setActiveRoute: (route: string) => void;
  setRecording: (recording: boolean, recordingId?: string | null) => void;
}

export const useAppStore = create<AppState>((set) => ({
  darkMode: false,
  activeRoute: '/',
  isRecording: false,
  recordingId: null,

  toggleDarkMode: () => set((s) => ({ darkMode: !s.darkMode })),
  setActiveRoute: (route) => set({ activeRoute: route }),
  setRecording: (recording, recordingId = null) =>
    set({ isRecording: recording, recordingId }),
}));
