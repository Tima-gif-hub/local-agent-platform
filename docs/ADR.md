# Architecture Decision Records

Compact registry of decisions that shape the codebase. New decisions append here;
superseding a decision requires a new ADR entry, never a silent edit.

---

## ADR-001: Desktop stack — Tauri 2 + React/TypeScript UI
The UI runs in a WebView with no OS access; only the Rust core can touch the system.
This process-level privilege boundary is a core part of the security model, and the
footprint stays small for an always-running assistant. Tauri's sidecar mechanism also
covers future out-of-process workers (Python, whisper).

## ADR-002: Core logic lives in workspace crates, not in `src-tauri`
`src-tauri` is a thin adapter (commands, window, tray). All logic is in `crates/*`
so it is testable without a GUI.

## ADR-003: Skills are in-process Rust behind the `Skill` trait; manifests are process-agnostic
The manifest contract (id, params schema, permissions, risk, examples) does not assume
an implementation language. Out-of-process skill runners (e.g. Python sidecars) arrive
in v0.3 using the same manifests. **Consequence:** there is deliberately no third-party
skill installation before process isolation exists.

## ADR-004: Plugins are skills installed from outside — one capability model
A "plugin" is a packaged, versioned set of skills with manifests. There is no separate
plugin concept: one permission model, one execution path, one set of docs.

## ADR-005: LLM strategy — hybrid routing, Ollama first, everything behind `LlmClient`
Routing order: rules → fuzzy match → LLM. Most commands never reach a model, which
keeps routing fast, testable, and offline-safe. Ollama is the first backend (detected
at startup, guided install during onboarding); model presets are exposed to users as
Fast / Balanced / Capable. All model HTTP lives in `crates/jarvis-llm` behind the
`LlmClient` trait; cloud providers can be added later as new implementations.

## ADR-006: Storage — a single SQLite database
One file (`jarvis.db`) holds settings, memory, audit log, and conversations — easy to
back up, export, or delete, which is the privacy story. Semantic memory arrives in
v0.3 via the `sqlite-vec` extension, not a separate vector database. All SQL lives in
`crates/jarvis-store`; migrations are checked in.

## ADR-007: Security model — declared permissions, risk levels, confirmation, audit
Manifests declare `permissions` and `risk`. The executor enforces: JSON Schema
validation of params, permission checks, user confirmation for `Moderate`+ risk
(threshold configurable), and an append-only audit log of every execution attempt,
including denials. LLM output is data, never code: a hallucinated skill id or
parameter is rejected, not attempted. Content read from files or the web is returned
to the user as data and is never fed back into the router as instructions.

## ADR-008: Multilingual strategy — canonical English inside, LLM at the edges
Intents, manifests, code, logs, and memory keys are English. User input in any
language goes through the same routing pipeline (fuzzy aliases + LLM handle language).
UI strings are localized via react-i18next (en + ru to start). Skill outputs produce
an English `summary`; replies are rendered in the user's language when an LLM is
available.

## ADR-009: Windows-first
Only Windows is a tested, supported target until beta. OS-specific code is confined
to skill implementations and a small `platform` module so other platforms can be
added later without core changes.

## ADR-010: Scheduler and watchers reuse the execution pipeline (v0.2)
Schedules and watchers are stored definitions that, when triggered, emit a stored
`InvocationPlan` into the same executor — same permissions, same audit log.
Unattended runs auto-execute only `Safe` skills; anything riskier notifies the user
and waits.
