import { create } from 'zustand';
import { invoke } from '@tauri-apps/api/core';
import type { Recording, Step, StepSave } from '../types';

// ============================================================
// Editor state (single-recording editing)
// ============================================================

export interface EditorState {
  currentRecording: Recording | null;
  steps: Step[];
  loading: boolean;

  loadRecording: (id: string) => Promise<void>;
  updateStep: (id: string, data: StepSave) => Promise<void>;
  deleteStep: (id: string) => Promise<void>;
  reorderSteps: (recordingId: string, stepIds: string[]) => Promise<void>;
  addStep: (
    recordingId: string,
    orderIndex: number,
    data: StepSave,
  ) => Promise<Step>;
}

export const useEditorStore = create<EditorState>((set, get) => ({
  currentRecording: null,
  steps: [],
  loading: false,

  loadRecording: async (id) => {
    set({ loading: true });
    try {
      const result = await invoke<{
        recording: Recording;
        steps: Step[];
      }>('load_recording', { id });
      set({
        currentRecording: result.recording,
        steps: result.steps,
        loading: false,
      });
    } catch {
      set({ loading: false });
    }
  },

  updateStep: async (id, data) => {
    await invoke('update_step', { id, data });
    set((s) => ({
      steps: s.steps.map((st) =>
        st.id === id ? { ...st, ...data } : st,
      ),
    }));
  },

  deleteStep: async (id) => {
    await invoke('delete_step', { id });
    set((s) => ({
      steps: s.steps.filter((st) => st.id !== id),
    }));
  },

  reorderSteps: async (recordingId, stepIds) => {
    await invoke('reorder_steps', { recordingId, stepIds });
    const current = get().steps;
    const reordered = stepIds
      .map((id, idx) => {
        const step = current.find((s) => s.id === id);
        return step ? { ...step, order_index: idx } : null;
      })
      .filter(Boolean) as Step[];
    set({ steps: reordered });
  },

  addStep: async (recordingId, orderIndex, data) => {
    const step = await invoke<Step>('add_step', {
      recordingId,
      orderIndex,
      data,
    });
    set((s) => {
      const steps = [...s.steps];
      steps.splice(orderIndex, 0, step);
      return {
        steps: steps.map((st, i) => ({ ...st, order_index: i })),
      };
    });
    return step;
  },
}));
