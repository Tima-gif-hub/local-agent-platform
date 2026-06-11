export type Risk = 'safe' | 'moderate' | 'destructive';
export type OllamaStatus = 'available' | 'model_missing' | 'down' | 'unknown';

export interface SkillInvocation {
  skill_id: string;
  params: Record<string, unknown>;
}

export interface InvocationPlan {
  steps: SkillInvocation[];
  source: string;
  confidence: number;
}

export interface PreviewDto {
  plan_id: string | null;
  plan: InvocationPlan | null;
  clarify: string | null;
  risk: Risk | null;
}

export interface StepReport {
  skill_id: string;
  risk: Risk;
  outcome: string;
  error: string | null;
  output: unknown | null;
}

export interface ReportDto {
  success: boolean;
  summary: string;
  report: {
    steps: StepReport[];
  };
}

export interface HistoryRow {
  id: number;
  ts: string;
  skill_id: string;
  risk: Risk;
  outcome: string;
  route_source: string;
  params: Record<string, unknown>;
}

export interface SettingsDto {
  language: 'en' | 'ru';
  confirm_threshold: Risk;
  auto_run_safe: boolean;
  model_preset: 'fast' | 'balanced' | 'capable';
  onboarding_done: boolean;
}

export interface ConfirmationRequest {
  id: string;
  prompt: string;
  skill_id?: string | null;
  params?: Record<string, unknown>;
  risk?: Risk | null;
}

export interface PullProgress {
  status: string;
  digest?: string | null;
  completed?: number | null;
  total?: number | null;
}
