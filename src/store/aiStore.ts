import { create } from 'zustand';
import { invoke } from '@tauri-apps/api/core';
import type { AiConfig } from '../types';

// ============================================================
// AI configuration & generation state
// ============================================================

export interface AiState {
  aiConfig: AiConfig | null;
  generating: boolean;
  progress: number;
  total: number;

  getAiConfig: () => Promise<void>;
  setAiConfig: (config: AiConfig) => Promise<void>;
  generateAiSteps: (recordingId: string) => Promise<void>;
}

export const useAiStore = create<AiState>((set, get) => ({
  aiConfig: null,
  generating: false,
  progress: 0,
  total: 0,

  getAiConfig: async () => {
    // Fetch config for the first available provider by scanning all ai_config_% keys
    const config = await invoke<AiConfig | null>('get_first_ai_config');
    set({ aiConfig: config });
  },

  setAiConfig: async (config) => {
    await invoke('set_ai_config', { config });
    set({ aiConfig: config });
  },

  generateAiSteps: async (recordingId) => {
    set({ generating: true, progress: 0 });
    try {
      await invoke('generate_ai_steps', { recordingId });
      set({ generating: false, progress: get().total });
    } catch {
      set({ generating: false });
    }
  },
}));
