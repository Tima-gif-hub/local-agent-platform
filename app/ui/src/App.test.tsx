import '@testing-library/jest-dom/vitest';
import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { App, PlanPreview } from './App';
import { Bridge, defaultSettings } from './bridge';
import en from './i18n/locales/en.json';
import ru from './i18n/locales/ru.json';
import type { ConfirmationRequest, PreviewDto } from './types';

describe('i18n', () => {
  it('keeps en and ru keys in parity', () => {
    expect(Object.keys(ru).sort()).toEqual(Object.keys(en).sort());
  });
});

describe('PlanPreview', () => {
  it('renders params readably', () => {
    const preview: PreviewDto = {
      plan_id: '1',
      risk: 'safe',
      clarify: null,
      plan: {
        source: 'Rule',
        confidence: 1,
        steps: [
          {
            skill_id: 'system.open_app',
            params: { name: 'Google Chrome', profile: 'Default' },
          },
        ],
      },
    };

    render(<PlanPreview preview={preview} onRun={() => undefined} onCancel={() => undefined} />);

    expect(screen.getByText('system.open_app')).toBeInTheDocument();
    expect(screen.getByText('name:')).toBeInTheDocument();
    expect(screen.getByText('Google Chrome')).toBeInTheDocument();
    expect(screen.getByText('profile:')).toBeInTheDocument();
  });
});

describe('confirmation flow', () => {
  it('responds through the event bridge', async () => {
    let confirmationHandler: ((request: ConfirmationRequest) => void) | undefined;
    const respondConfirmation = vi.fn().mockResolvedValue(undefined);
    const bridge: Bridge = {
      routeAndPreview: vi.fn(),
      execute: vi.fn(),
      history: vi.fn().mockResolvedValue({ rows: [] }),
      getSettings: vi.fn().mockResolvedValue({ ...defaultSettings, onboarding_done: true }),
      saveSettings: vi.fn().mockImplementation(async (settings) => settings),
      ollamaStatus: vi.fn().mockResolvedValue({ status: 'available', model: 'qwen2.5:3b' }),
      pullSelectedModel: vi.fn(),
      respondConfirmation,
      onConfirmation: vi.fn().mockImplementation(async (handler) => {
        confirmationHandler = handler;
        return () => undefined;
      }),
      onPullProgress: vi.fn().mockResolvedValue(() => undefined),
      hideWindow: vi.fn(),
    };

    render(<App bridge={bridge} />);
    await waitFor(() => expect(confirmationHandler).toBeDefined());
    act(() => {
      confirmationHandler?.({
        id: '42',
        prompt: '',
        skill_id: 'files.convert_images',
        params: { folder: 'C:/pics' },
        risk: 'moderate',
      });
    });

    expect(await screen.findByText('Confirm action')).toBeInTheDocument();
    fireEvent.click(screen.getByText('Deny'));

    expect(respondConfirmation).toHaveBeenCalledWith('42', false);
  });
});
