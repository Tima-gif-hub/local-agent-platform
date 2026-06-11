# Roadmap

| Phase | Theme | Adds | Does NOT add |
|---|---|---|---|
| **v0.1 MVP** | "It executes commands safely" | see docs/MVP.md | everything else |
| **v0.2** | Automation | scheduler, watchers, routines (all reuse `InvocationPlan`), notification center, +10 skills (git, docker, pdf, clipboard) | plugins, voice |
| **v0.3** | Memory & extensibility | sqlite-vec semantic memory + memory manager UI, out-of-process skill runner (Python sidecar), skill dev template + docs | marketplace |
| **v0.4** | Voice & polish | whisper.cpp push-to-talk STT (multilingual), optional TTS, auto-updater, installer polish, macOS support | wake word |
| **v1.0** | Platform | third-party skill packages ("plugins") with isolation + trust prompts, skill catalog (git-based, not a marketplace), Linux support, cloud LLM providers | multi-agent |
| **v2+** | Agentic | multi-step planning, reflection, background agents — only on top of proven v1 primitives | — |

## Phase gates
A phase starts only when the previous phase's success criteria are met. Any change to
a frozen (🔒) interface must be recorded as an ADR before the work begins. Features
from a later phase are not accepted early — see `docs/VISION.md §2`.
