import { create } from 'zustand';
import { invoke } from '@tauri-apps/api/core';
import type { Recording, RecordingSave } from '../types';

// ============================================================
// Recording list state
// ============================================================

export interface RecordingState {
  recordings: Recording[];
  loading: boolean;

  fetchRecordings: () => Promise<void>;
  deleteRecording: (id: string) => Promise<void>;
  saveRecording: (data: RecordingSave) => Promise<Recording>;
  startRecording: () => Promise<string>;
  finishRecording: (id: string) => Promise<void>;
}

export const useRecordingStore = create<RecordingState>((set, get) => ({
  recordings: [],
  loading: false,

  fetchRecordings: async () => {
    set({ loading: true });
    try {
      const recordings = await invoke<Recording[]>('list_recordings');
      set({ recordings, loading: false });
    } catch {
      set({ loading: false });
    }
  },

  deleteRecording: async (id) => {
    await invoke('delete_recording', { id });
    set((s) => ({
      recordings: s.recordings.filter((r) => r.id !== id),
    }));
  },

  saveRecording: async (data) => {
    const recording = await invoke<Recording>('save_recording', { data });
    set((s) => ({ recordings: [...s.recordings, recording] }));
    return recording;
  },

  startRecording: async () => {
    const id = await invoke<string>('start_recording');
    return id;
  },

  finishRecording: async (id) => {
    await invoke('finish_recording', { id });
  },
}));
