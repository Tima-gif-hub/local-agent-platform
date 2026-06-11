import {
  FormEvent,
  KeyboardEvent,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from 'react';
import { useTranslation } from 'react-i18next';
import './i18n';
import { Bridge, defaultSettings, tauriBridge } from './bridge';
import type {
  ConfirmationRequest,
  HistoryRow,
  OllamaStatus,
  PreviewDto,
  PullProgress,
  ReportDto,
  Risk,
  SettingsDto,
} from './types';

type View = 'palette' | 'history' | 'settings' | 'onboarding';
type FeedItem =
  | { kind: 'user'; text: string }
  | { kind: 'clarify'; text: string }
  | { kind: 'preview'; preview: PreviewDto }
  | { kind: 'result'; report: ReportDto };

interface AppProps {
  bridge?: Bridge;
}

export function App({ bridge = tauriBridge }: AppProps) {
  const { t, i18n } = useTranslation();
  const [input, setInput] = useState('');
  const [view, setView] = useState<View>('palette');
  const [feed, setFeed] = useState<FeedItem[]>([]);
  const [settings, setSettings] = useState<SettingsDto>(defaultSettings);
  const [status, setStatus] = useState<OllamaStatus>('unknown');
  const [statusModel, setStatusModel] = useState('');
  const [busy, setBusy] = useState(false);
  const [confirmation, setConfirmation] = useState<ConfirmationRequest | null>(null);
  const [historyRows, setHistoryRows] = useState<HistoryRow[]>([]);
  const [historyPage, setHistoryPage] = useState(0);
  const [historyOutcome, setHistoryOutcome] = useState('');
  const [onboardingStep, setOnboardingStep] = useState(0);
  const [pullProgress, setPullProgress] = useState<PullProgress | null>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  const feedRef = useRef<HTMLDivElement>(null);

  const refreshStatus = useCallback(async () => {
    try {
      const next = await bridge.ollamaStatus();
      setStatus(next.status);
      setStatusModel(next.model);
    } catch {
      setStatus('down');
    }
  }, [bridge]);

  useEffect(() => {
    void bridge
      .getSettings()
      .then((next) => {
        setSettings(next);
        void i18n.changeLanguage(next.language);
        if (!next.onboarding_done) {
          setView('onboarding');
        }
      })
      .catch(() => undefined);
    void refreshStatus();
    void bridge.onConfirmation(setConfirmation);
    void bridge.onPullProgress(setPullProgress);
  }, [bridge, i18n, refreshStatus]);

  useEffect(() => {
    const feedElement = feedRef.current;
    if (!feedElement) {
      return;
    }
    if (typeof feedElement.scrollTo === 'function') {
      feedElement.scrollTo({ top: feedElement.scrollHeight });
    } else {
      feedElement.scrollTop = feedElement.scrollHeight;
    }
  }, [feed]);

  async function submit(event: FormEvent) {
    event.preventDefault();
    const text = input.trim();
    if (!text || busy) {
      return;
    }
    setInput('');
    setBusy(true);
    setFeed((items) => [...items, { kind: 'user', text }]);
    try {
      const preview = await bridge.routeAndPreview(text);
      if (preview.clarify) {
        setFeed((items) => [...items, { kind: 'clarify', text: preview.clarify ?? '' }]);
      } else if (
        preview.plan_id &&
        preview.risk === 'safe' &&
        settings.auto_run_safe
      ) {
        const report = await bridge.execute(preview.plan_id);
        setFeed((items) => [...items, { kind: 'result', report }]);
      } else {
        setFeed((items) => [...items, { kind: 'preview', preview }]);
      }
    } catch (error) {
      setFeed((items) => [
        ...items,
        {
          kind: 'result',
          report: failedReport(error instanceof Error ? error.message : String(error)),
        },
      ]);
    } finally {
      setBusy(false);
    }
  }

  async function runPlan(planId: string) {
    if (busy) {
      return;
    }
    setBusy(true);
    try {
      const report = await bridge.execute(planId);
      setFeed((items) => items.filter((item) => item.kind !== 'preview').concat({ kind: 'result', report }));
    } finally {
      setBusy(false);
    }
  }

  const loadHistory = useCallback(
    async (page = 0, append = false, outcome = historyOutcome) => {
      const result = await bridge.history(page, outcome || undefined);
      setHistoryRows((rows) => (append ? [...rows, ...result.rows] : result.rows));
      setHistoryPage(page);
    },
    [bridge, historyOutcome],
  );

  async function saveSettings(next: SettingsDto) {
    const saved = await bridge.saveSettings(next);
    setSettings(saved);
    void i18n.changeLanguage(saved.language);
    void refreshStatus();
  }

  async function finishOnboarding(skip = false) {
    await saveSettings({ ...settings, onboarding_done: true });
    if (!skip && status === 'model_missing') {
      await pullModel();
    }
    setView('palette');
  }

  async function pullModel() {
    setPullProgress({ status: statusModel });
    await bridge.pullSelectedModel();
    await refreshStatus();
  }

  function keyDown(event: KeyboardEvent) {
    if (event.key === 'Escape') {
      if (confirmation) {
        void bridge.respondConfirmation(confirmation.id, false);
        setConfirmation(null);
      } else if (view !== 'palette') {
        setView('palette');
      } else {
        void bridge.hideWindow().catch(() => undefined);
      }
    }
    if (event.ctrlKey && event.key.toLowerCase() === 'h') {
      event.preventDefault();
      setView('history');
      void loadHistory(0);
    }
    if (event.key === 'Tab' && input.trim() === '') {
      event.preventDefault();
      setView('history');
      void loadHistory(0);
    }
  }

  const offline = status !== 'available';

  return (
    <main className="shell" onKeyDown={keyDown}>
      <form className="inputRow" onSubmit={submit}>
        <input
          ref={inputRef}
          autoFocus
          value={input}
          onChange={(event) => setInput(event.target.value)}
          placeholder={t('palette.placeholder')}
        />
        <span
          className={`statusDot ${status}`}
          title={`${t(`status.${status}`)} ${statusModel}`.trim()}
        />
      </form>
      {offline && <div className="offlineBanner">{t('banner.llm_offline')}</div>}
      {view === 'palette' && (
        <div className="feed" ref={feedRef}>
          {feed.map((item, index) => (
            <FeedCard
              key={index}
              item={item}
              onRun={runPlan}
              onCancel={() =>
                setFeed((items) => items.filter((candidate) => candidate !== item))
              }
            />
          ))}
        </div>
      )}
      {view === 'history' && (
        <HistoryView
          rows={historyRows}
          outcome={historyOutcome}
          onOutcomeChange={(value) => {
            setHistoryOutcome(value);
            void loadHistory(0, false, value);
          }}
          onLoadMore={() => void loadHistory(historyPage + 1, true)}
          onBack={() => setView('palette')}
        />
      )}
      {view === 'settings' && (
        <SettingsView
          settings={settings}
          status={status}
          model={statusModel}
          pullProgress={pullProgress}
          onChange={(next) => void saveSettings(next)}
          onPull={() => void pullModel()}
          onBack={() => setView('palette')}
        />
      )}
      {view === 'onboarding' && (
        <Onboarding
          step={onboardingStep}
          settings={settings}
          status={status}
          model={statusModel}
          pullProgress={pullProgress}
          onStep={setOnboardingStep}
          onSettings={(next) => void saveSettings(next)}
          onRecheck={() => void refreshStatus()}
          onPull={() => void pullModel()}
          onSkip={() => void finishOnboarding(true)}
          onDone={() => void finishOnboarding(false)}
        />
      )}
      <button
        className="gear"
        type="button"
        title={t('view.settings')}
        onClick={() => setView('settings')}
      >
        ⚙
      </button>
      {confirmation && (
        <ConfirmationDialog
          request={confirmation}
          onRespond={(accepted) => {
            void bridge.respondConfirmation(confirmation.id, accepted);
            setConfirmation(null);
          }}
        />
      )}
    </main>
  );
}

function FeedCard({
  item,
  onRun,
  onCancel,
}: {
  item: FeedItem;
  onRun: (planId: string) => void;
  onCancel: () => void;
}) {
  if (item.kind === 'user') {
    return (
      <div className="userLine">
        <span>&gt;</span>
        {item.text}
      </div>
    );
  }
  if (item.kind === 'clarify') {
    return <div className="card">{item.text}</div>;
  }
  if (item.kind === 'preview') {
    return (
      <PlanPreview
        preview={item.preview}
        onRun={() => item.preview.plan_id && onRun(item.preview.plan_id)}
        onCancel={onCancel}
      />
    );
  }
  return <ResultCard report={item.report} />;
}

export function PlanPreview({
  preview,
  onRun,
  onCancel,
}: {
  preview: PreviewDto;
  onRun: () => void;
  onCancel: () => void;
}) {
  const { t } = useTranslation();
  const step = preview.plan?.steps[0];
  if (!step) {
    return null;
  }
  return (
    <div className="card planPreview">
      <div className="row">
        <code>{step.skill_id}</code>
        <RiskBadge risk={preview.risk ?? 'safe'} />
      </div>
      <ParamList params={step.params} />
      <div className="actions">
        <button className="primary" type="button" onClick={onRun}>
          {t('actions.run')}
        </button>
        <button className="secondary" type="button" onClick={onCancel}>
          {t('actions.cancel')}
        </button>
      </div>
    </div>
  );
}

function ConfirmationDialog({
  request,
  onRespond,
}: {
  request: ConfirmationRequest;
  onRespond: (accepted: boolean) => void;
}) {
  const { t } = useTranslation();
  const denyRef = useRef<HTMLButtonElement>(null);
  const [seconds, setSeconds] = useState(60);
  useEffect(() => {
    denyRef.current?.focus();
    const timer = window.setInterval(() => {
      setSeconds((value) => {
        if (value <= 1) {
          onRespond(false);
          window.clearInterval(timer);
          return 0;
        }
        return value - 1;
      });
    }, 1000);
    return () => window.clearInterval(timer);
  }, [onRespond]);
  return (
    <div className="modal" role="dialog" aria-modal="true">
      <div className="modalCard">
        <div className="row">
          <h2>{t('confirm.title')}</h2>
          <RiskBadge risk={request.risk ?? 'moderate'} />
        </div>
        <code>{request.skill_id ?? request.prompt}</code>
        <ParamList params={request.params ?? {}} />
        <div className="actions">
          <button ref={denyRef} className="secondary" type="button" onClick={() => onRespond(false)}>
            {t('actions.deny')}
          </button>
          <button className="primary" type="button" onClick={() => onRespond(true)}>
            {t('actions.confirm')}
          </button>
        </div>
        <div className="countdown" style={{ transform: `scaleX(${seconds / 60})` }} />
      </div>
    </div>
  );
}

function ResultCard({ report }: { report: ReportDto }) {
  const { t } = useTranslation();
  const first = report.report.steps[0];
  return (
    <div className={`card result ${report.success ? '' : 'failed'}`}>
      <div className="row">
        <span>{report.summary || t(report.success ? 'result.success' : 'result.failure')}</span>
        {first && <RiskBadge risk={first.risk} />}
      </div>
      {first?.error && <p className="errorText">{first.error}</p>}
      <details>
        <summary>{t('details')}</summary>
        <pre>{JSON.stringify(first?.output ?? report.report.steps, null, 2)}</pre>
      </details>
    </div>
  );
}

function HistoryView({
  rows,
  outcome,
  onOutcomeChange,
  onLoadMore,
  onBack,
}: {
  rows: HistoryRow[];
  outcome: string;
  onOutcomeChange: (value: string) => void;
  onLoadMore: () => void;
  onBack: () => void;
}) {
  const { t } = useTranslation();
  return (
    <section className="panel">
      <div className="row">
        <h1>{t('history.title')}</h1>
        <select value={outcome} onChange={(event) => onOutcomeChange(event.target.value)}>
          <option value="">{t('history.filter.all')}</option>
          {['success', 'denied_permission', 'denied_confirmation', 'invalid_params', 'failed'].map(
            (value) => (
              <option key={value} value={value}>
                {value}
              </option>
            ),
          )}
        </select>
      </div>
      <div className="historyRows">
        {rows.length === 0 && <div className="empty">{t('history.empty')}</div>}
        {rows.map((row) => (
          <details className="historyRow" key={row.id}>
            <summary>
              <span>{row.ts}</span>
              <code>{row.skill_id}</code>
              <OutcomeBadge outcome={row.outcome} />
            </summary>
            <pre>{JSON.stringify(row.params, null, 2)}</pre>
          </details>
        ))}
      </div>
      <div className="actions">
        <button className="secondary" type="button" onClick={onBack}>
          {t('actions.back')}
        </button>
        <button className="secondary" type="button" onClick={onLoadMore}>
          {t('actions.load_more')}
        </button>
      </div>
    </section>
  );
}

function SettingsView({
  settings,
  status,
  model,
  pullProgress,
  onChange,
  onPull,
  onBack,
}: {
  settings: SettingsDto;
  status: OllamaStatus;
  model: string;
  pullProgress: PullProgress | null;
  onChange: (settings: SettingsDto) => void;
  onPull: () => void;
  onBack: () => void;
}) {
  const { t } = useTranslation();
  return (
    <section className="panel settingsPanel">
      <h1>{t('settings.title')}</h1>
      <label>
        {t('settings.language')}
        <select
          value={settings.language}
          onChange={(event) => onChange({ ...settings, language: event.target.value as 'en' | 'ru' })}
        >
          <option value="en">en</option>
          <option value="ru">ru</option>
        </select>
      </label>
      <label>
        {t('settings.confirm_threshold')}
        <select
          value={settings.confirm_threshold}
          onChange={(event) => onChange({ ...settings, confirm_threshold: event.target.value as Risk })}
        >
          {(['safe', 'moderate', 'destructive'] as Risk[]).map((risk) => (
            <option key={risk} value={risk}>
              {risk}
            </option>
          ))}
        </select>
      </label>
      <label className="check">
        <input
          type="checkbox"
          checked={settings.auto_run_safe}
          onChange={(event) => onChange({ ...settings, auto_run_safe: event.target.checked })}
        />
        {t('settings.auto_run_safe')}
      </label>
      <fieldset>
        <legend>{t('settings.model_preset')}</legend>
        {(['fast', 'balanced', 'capable'] as SettingsDto['model_preset'][]).map((preset) => (
          <label className="radioLine" key={preset}>
            <input
              type="radio"
              checked={settings.model_preset === preset}
              onChange={() => onChange({ ...settings, model_preset: preset })}
            />
            <span>{preset}</span>
          </label>
        ))}
      </fieldset>
      <div className="statusLine">
        <span className={`statusDot ${status}`} />
        <span>{t('settings.ollama_status')}</span>
        <code>{model || t(`status.${status}`)}</code>
      </div>
      {pullProgress && <Progress progress={pullProgress} />}
      <div className="actions">
        <button className="secondary" type="button" onClick={onBack}>
          {t('actions.back')}
        </button>
        <button className="primary" type="button" onClick={onPull}>
          {t('actions.pull_model')}
        </button>
      </div>
    </section>
  );
}

function Onboarding({
  step,
  settings,
  status,
  model,
  pullProgress,
  onStep,
  onSettings,
  onRecheck,
  onPull,
  onSkip,
  onDone,
}: {
  step: number;
  settings: SettingsDto;
  status: OllamaStatus;
  model: string;
  pullProgress: PullProgress | null;
  onStep: (step: number) => void;
  onSettings: (settings: SettingsDto) => void;
  onRecheck: () => void;
  onPull: () => void;
  onSkip: () => void;
  onDone: () => void;
}) {
  const { t } = useTranslation();
  const titles = ['onboarding.welcome.title', 'onboarding.ollama.title', 'onboarding.preset.title'];
  const bodies = ['onboarding.welcome.body', 'onboarding.ollama.body', 'onboarding.preset.body'];
  return (
    <section className="onboarding card">
      <div className="dots">
        {[0, 1, 2].map((index) => (
          <span key={index} className={index === step ? 'active' : ''} />
        ))}
      </div>
      <h1>{t(titles[step])}</h1>
      <p>{t(bodies[step])}</p>
      {step === 1 && (
        <>
          <div className="statusLine">
            <span className={`statusDot ${status}`} />
            <code>{model || t(`status.${status}`)}</code>
          </div>
          {pullProgress && <Progress progress={pullProgress} />}
        </>
      )}
      {step === 2 && (
        <fieldset>
          <legend>{t('settings.model_preset')}</legend>
          {(['fast', 'balanced', 'capable'] as SettingsDto['model_preset'][]).map((preset) => (
            <label className="radioLine" key={preset}>
              <input
                type="radio"
                checked={settings.model_preset === preset}
                onChange={() => onSettings({ ...settings, model_preset: preset })}
              />
              <span>{preset}</span>
            </label>
          ))}
        </fieldset>
      )}
      <div className="actions split">
        <button className="ghost" type="button" onClick={onSkip}>
          {t('actions.skip')}
        </button>
        {step === 1 && (
          <button className="secondary" type="button" onClick={onRecheck}>
            {t('actions.recheck')}
          </button>
        )}
        {step === 1 && status === 'model_missing' && (
          <button className="secondary" type="button" onClick={onPull}>
            {t('actions.pull_model')}
          </button>
        )}
        <button
          className="primary"
          type="button"
          onClick={() => (step === 2 ? onDone() : onStep(step + 1))}
        >
          {step === 2 ? t('actions.save') : t('actions.run')}
        </button>
      </div>
    </section>
  );
}

function Progress({ progress }: { progress: PullProgress }) {
  const ratio = useMemo(() => {
    if (!progress.completed || !progress.total) {
      return 0.15;
    }
    return Math.max(0.05, Math.min(1, progress.completed / progress.total));
  }, [progress]);
  return (
    <div className="progress" title={progress.status}>
      <span style={{ transform: `scaleX(${ratio})` }} />
    </div>
  );
}

function ParamList({ params }: { params: Record<string, unknown> }) {
  const { t } = useTranslation();
  const entries = Object.entries(params);
  if (entries.length === 0) {
    return <code className="params">{t('params.empty')}</code>;
  }
  return (
    <dl className="params">
      {entries.map(([key, value]) => (
        <div key={key}>
          <dt>{key}:</dt>
          <dd>{truncateMiddle(formatParam(value))}</dd>
        </div>
      ))}
    </dl>
  );
}

function RiskBadge({ risk }: { risk: Risk }) {
  return <span className={`badge risk-${risk}`}>{risk}</span>;
}

function OutcomeBadge({ outcome }: { outcome: string }) {
  const tone = outcome === 'success' ? 'safe' : outcome.startsWith('denied') ? 'moderate' : 'destructive';
  return <span className={`badge risk-${tone}`}>{outcome}</span>;
}

function formatParam(value: unknown) {
  return typeof value === 'string' ? value : JSON.stringify(value);
}

function truncateMiddle(value: string) {
  if (value.length <= 60) {
    return value;
  }
  return `${value.slice(0, 28)}...${value.slice(-28)}`;
}

function failedReport(message: string): ReportDto {
  return {
    success: false,
    summary: message,
    report: {
      steps: [
        {
          skill_id: 'ui.error',
          risk: 'destructive',
          outcome: 'failed',
          error: message,
          output: null,
        },
      ],
    },
  };
}
