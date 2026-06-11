# Jarvis — Vision

## 1. Product Statement

**Jarvis is a local-first desktop assistant that turns natural language (any language)
into safe, auditable actions on your computer, and is extended by adding skills —
not by modifying the core.**

The long-term direction is a personal AI operating layer: the user interacts with
their computer through natural language, and Jarvis routes intent to skills, executes
them under a permission model, remembers context, and automates workflows.

How Jarvis differs from existing tools:
- vs **Raycast/Alfred**: natural language + LLM routing, Windows-first, open skill model.
- vs **Open Interpreter / AutoGPT**: the model never executes code; it only selects
  declared skills with validated parameters. Safety is structural, not prompt-based.
- vs **ChatGPT/Claude Desktop**: local-first, OS-action-first, works offline with Ollama.

## 2. Phased Scope

Jarvis is built in deliberate phases (see `docs/ROADMAP.md`). Each phase has a frozen
scope; features belonging to later phases are not accepted early, even good ones —
the current phase's scope is defined in `docs/MVP.md`. The platform primitives
(skill manifests, the routing pipeline, the execution pipeline) are designed so that
later capabilities — routines, watchers, out-of-process skills, plugins, semantic
memory, voice — layer on top of stable interfaces instead of requiring rewrites.

## 3. Non-Negotiable Principles (stable forever)
1. The LLM proposes; the platform executes. No raw shell access for the model.
2. Every skill declares a manifest: id, params schema, permissions, risk level.
3. Risky actions require user confirmation; everything executed is audit-logged.
4. Local-first: full core functionality with Ollama offline. Cloud LLMs optional.
5. Canonical internal language is English (intents, manifests, code, logs).
   User-facing language is whatever the user speaks — translation is the LLM's job.
6. Adding a skill must never require modifying the router, UI, or core.
