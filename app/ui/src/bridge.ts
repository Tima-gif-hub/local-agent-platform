import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { getCurrentWindow } from '@tauri-apps/api/window';
import type {
  ConfirmationRequest,
  HistoryRow,
  OllamaStatus,
  PreviewDto,
  PullProgress,
  ReportDto,
  SettingsDto,
} from './types';

export interface Bridge {
  routeAndPreview(text: string): Promise<PreviewDto>;
  execute(planId: string): Promise<ReportDto>;
  history(page: number, outcome?: string): Promise<{ rows: HistoryRow[] }>;
  getSettings(): Promise<SettingsDto>;
  saveSettings(settings: SettingsDto): Promise<SettingsDto>;
  ollamaStatus(): Promise<{ status: OllamaStatus; model: string }>;
  pullSelectedModel(): Promise<void>;
  respondConfirmation(id: string, accepted: boolean): Promise<void>;
  onConfirmation(handler: (request: ConfirmationRequest) => void): Promise<() => void>;
  onPullProgress(handler: (progress: PullProgress) => void): Promise<() => void>;
  hideWindow(): Promise<void>;
}

export const tauriBridge: Bridge = {
  routeAndPreview: (text) => invoke('route_and_preview', { text }),
  execute: (planId) => invoke('execute', { planId }),
  history: (page, outcome) => invoke('history', { page, outcome }),
  getSettings: () => invoke('get_settings'),
  saveSettings: (settings) => invoke('save_settings', { settings }),
  ollamaStatus: () => invoke('ollama_status'),
  pullSelectedModel: () => invoke('pull_selected_model'),
  respondConfirmation: (id, accepted) =>
    invoke('respond_confirmation', { response: { id, accepted } }),
  onConfirmation: async (handler) => {
    const unlisten = await listen<ConfirmationRequest>('confirmation-requested', (event) =>
      handler(event.payload),
    );
    return unlisten;
  },
  onPullProgress: async (handler) => {
    const unlisten = await listen<PullProgress>('pull-progress', (event) =>
      handler(event.payload),
    );
    return unlisten;
  },
  hideWindow: async () => {
    await getCurrentWindow().hide();
  },
};

export const defaultSettings: SettingsDto = {
  language: 'en',
  confirm_threshold: 'moderate',
  auto_run_safe: false,
  model_preset: 'fast',
  onboarding_done: false,
};
