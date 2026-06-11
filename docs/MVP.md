# MVP Scope (v0.1) — frozen

**Definition of done:** a Windows user can press a global hotkey, type
"open chrome" / "открой vs code" / "convert png files in this folder to jpg",
see what Jarvis plans to do, confirm if needed, and it happens — fully offline
with Ollama, with every action visible in History.

## In scope
1. **Tauri app**: single window (chat + command palette in one input), tray icon,
   global hotkey (Alt+Space), settings page, history page.
2. **Router**: rules → fuzzy → Ollama JSON routing → clarify. Deterministic test suite
   with a mock LLM.
3. **Skills (8, in-process Rust):**
   - `system.open_app` — launch installed application by name (moderate fuzzy app lookup)
   - `files.open_folder` — open folder in Explorer
   - `files.search` — find files by name pattern under a root
   - `files.convert_images` — png↔jpg/webp in a folder (Destructive=no, Moderate: writes files)
   - `system.info` — CPU/RAM/disk snapshot with human summary
   - `system.processes` — top processes by CPU/RAM
   - `web.open_url` — open URL in default browser
   - `memory.remember` / `memory.recall` — structured facts ("my projects folder is D:\dev")
4. **Permissions & safety**: risk levels, confirmation dialog with parameter preview,
   append-only audit log, History UI reading it.
5. **Storage**: SQLite with migrations; settings, memory_facts, audit_log, conversations.
6. **Onboarding (minimal)**: detect Ollama → if missing, guided install link + model pull
   with progress; pick Fast/Balanced/Capable preset. No bundled runtime.
7. **i18n**: UI strings en+ru; input in any language via router.

## Explicitly OUT of scope (do not build, do not stub)
Voice, TTS, watchers, scheduler, routines, plugins, marketplace, vector memory,
multi-step plans, cloud LLMs, auto-update, macOS/Linux testing, telemetry.

## Success criteria
- 20 canonical utterances (10 en, 10 ru) route correctly: ≥18 via rules/fuzzy/LLM,
  0 misroutes to a Destructive skill without confirmation.
- Cold start < 2s; idle RAM of the app (excl. Ollama) < 150 MB.
- `cargo test` covers router and executor without a GUI or live LLM.
