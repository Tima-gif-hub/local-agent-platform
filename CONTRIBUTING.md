# Contributing to Jarvis

This document covers setup, conventions, and the rules that every pull request must
respect. Read `docs/ARCHITECTURE.md` and `docs/ADR.md` before writing any code.

---

## Prerequisites

| Tool | Version | Notes |
|---|---|---|
| Rust | stable (latest) | Install via [rustup](https://rustup.rs) |
| Node.js | 20+ | Required for the React/Vite UI |
| npm | bundled with Node | Used for UI deps and `tauri dev` |
| WebView2 runtime | any | Shipped with Windows 11; on Windows 10, install from Microsoft |
| Visual Studio C++ build tools | 2019+ | Required by Tauri 2 on Windows |
| Ollama | any | Optional at build time; required at runtime for LLM routing |

Full Tauri 2 Windows prerequisites:
<https://v2.tauri.app/start/prerequisites/>

---

## Build, test, and lint

```powershell
# Build the Rust workspace
cargo build

# Build the UI
cd app/ui && npm install && npm run build

# Run the full app in dev mode (Rust + Vite hot-reload)
cd app && npm run tauri dev

# Run all tests
cargo test

# Lint (zero warnings required)
cargo clippy -- -D warnings
```

Tests must pass and clippy must be clean in every crate you touch before opening a PR.
The UI build must also succeed without errors.

---

## Branch naming

```
feat/short-description  # new functionality
fix/short-description   # bug fix
docs/short-description  # documentation only
```

Do not commit directly to `main`.

---

## Commit conventions

Jarvis uses [Conventional Commits](https://www.conventionalcommits.org/):

```
feat(router): add trigram fuzzy fallback before LLM step
fix(executor): reject params with extra fields not in schema
docs(adr): record ADR-012 for X
test(skills): add tempdir test for files.convert_images
refactor(store): extract audit writer into its own module
```

Scope is the crate name or subsystem (`router`, `skills`, `store`, `llm`, `ui`, `executor`).
Breaking changes must include `BREAKING CHANGE:` in the footer.

---

## Contribution workflow

1. **Open or pick an issue first.** Check it fits the current roadmap phase
   (`docs/ROADMAP.md`); features from later phases will be declined for now, even
   good ones — that is scope discipline, not a judgment of the idea.
2. **Read** `docs/ARCHITECTURE.md`, `docs/ADR.md`, and `docs/MVP.md` before writing
   code. They take precedence over anything else.
3. **Branch** from `main`, implement, and make sure: `cargo build` and `cargo test`
   are green, `cargo clippy -- -D warnings` is clean in touched crates, and the UI
   builds without TypeScript errors.
4. **Do not** add Cargo/npm dependencies without explaining why in the PR description.
   Do not change frozen (🔒) interfaces without proposing an ADR. Do not build
   anything on the MVP out-of-scope list.
5. **Open a PR** describing what and why; a maintainer reviews for acceptance and
   architecture conformance before merge.

---

## Architecture boundaries that PRs must respect

These rules come from `docs/ARCHITECTURE.md` and `docs/ADR.md`. Violating them will
cause the PR to be rejected.

- **LLM never executes.** The LLM produces a schema-validated `InvocationPlan`. The
  executor runs it. No raw shell strings, eval, or dynamic dispatch based on unvalidated
  model output.
- **Skills use `SkillContext`, not `std::fs`/`std::process`.** Every OS interaction from
  a skill must go through `SkillContext` helpers, which are permission-checked. Direct use
  of `std::fs` or `std::process` in `jarvis-skills` will be rejected.
- **Audit log is append-only.** Every execution attempt — including denials and failures —
  must be recorded. No update or delete of audit rows.
- **SQL lives in `jarvis-store`.** No SQL strings in any other crate.
- **LLM HTTP lives in `jarvis-llm`.** No direct HTTP to a model provider from any other
  crate.
- **Canonical language is English.** Code, manifests, log messages, and skill `summary`
  fields are English. UI-visible strings go through `react-i18next` (en + ru catalogs).
- **Windows-first.** Until beta, only Windows is a tested target. OS-specific code is
  confined to skill implementations and the `platform` module.
- **Frozen interfaces (🔒).** `jarvis-types` (core data types and `Skill` trait), the
  routing pipeline signature, and the execution pipeline signature are frozen. Changes
  require a new ADR entry in `docs/ADR.md` before any code changes.

---

## Testing expectations

- **No test may require a live LLM or a running GUI.** The `jarvis-llm` crate exposes
  `MockLlm` implementing `LlmClient`; all router tests use it.
- **No test may require network access.** External calls must be behind a trait with a
  test double.
- **Skill tests** use a temporary directory (`tempfile::TempDir`). They must clean up
  after themselves.
- **Parameter validation** must be tested for every skill: valid params succeed, invalid
  params (wrong type, missing required field, extra field) return a schema error without
  touching the OS.
- **Executor tests** must cover: schema validation rejection, permission denial, risk
  confirmation path, and audit log entries for each outcome.

---

## Proposing an ADR

Architectural decisions — anything that changes a frozen interface, adds a significant
dependency, alters the security model, or makes a non-obvious trade-off — require an ADR.

1. Open `docs/ADR.md`.
2. Append a new section using the compact format already in that file:
   ```
   ## ADR-NNN: Short title
   **Decision:** ...
   **Alternatives:** ...
   **Why:** ...
   **Risk:** ...
   ```
3. Reference the ADR number in the commit message:
   ```
   docs(adr): record ADR-012 for <topic>
   ```
4. Link to the ADR in the PR description.

Do not silently edit existing ADRs. If a previous decision is superseded, add a new ADR
that explicitly overrides it and note the supersession in both entries.
