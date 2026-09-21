# PrintPilot — 3D Printing Project Management Platform

**English** | [简体中文](README.zh-CN.md)

PrintPilot is a desktop platform that takes a 3D-printed product from an idea to a listing, and keeps the whole process measurable:

**market research → product imagery → parametric 3D model → prototyping & pricing → publishing → operations → review**

AI drafts; a person decides. Every stage has an explicit checklist, computed from what has actually been produced, so a product only moves forward when the evidence is there.

## Highlights

- **Project board** — one card per product, one column per stage. Stage gates are *computed* from the project's outputs (research reports, adopted images, models, print runs, a chosen price), not ticked by hand. Skipping a gate asks for a reason that is kept for later review.
- **Market research agent** — a bounded pipeline (plan → search → score → self-review) over an LLM and a web-search provider. Every opportunity carries its evidence and a price band.
- **Image workbench** — conversational image generation and instruction-based editing for reference renders, scene shots and covers. Any image can be sent to modeling in one click.
- **Modeling studio** — reference image or a sentence → a reviewed design spec → a parametric [build123d](https://github.com/gumyr/build123d) script → executed in a sandbox on a real CAD kernel (OpenCascade) → automatic checks and a bounded repair loop. Then keep talking to refine it: parameter edits rebuild locally in about 0.1–0.3 s and cost nothing; conversational edits touch only the relevant feature section; every change is a version you can return to. Exports STEP, STL and 3MF, and reports printability (bed contact, flat overhangs).
- **Cost & pricing** — a transparent cost breakdown (material, machine time, labour, packaging, channel fees; failed prints are amortised into both material *and* machine time), three suggested price tiers, profit per machine hour, capacity, and a print-run log.
- **Native-feeling UI** — context menus for every object, streaming AI replies, and any running turn can be cancelled.

Planned next: publishing packs (semi-automatic listing), content calendar, orders & printing, analytics. See the [roadmap](docs/06-roadmap.md).

## Design principles

- **One installer, zero external dependencies.** A single setup executable; nothing else to install. The CAD engine (Python + build123d + OpenCascade, about 224 MB compressed) ships inside the installer and is unpacked on first use, offline. The database is embedded SQLite.
- **Local-first.** All data stays on the user's machine. The asset library can live on any drive.
- **Bring your own API keys.** Keys are entered in Settings and stored only in the operating system's credential manager — never in configuration files, logs or the database.
- **No unofficial platform automation.** Publishing is deliberately semi-automatic: the application prepares everything, a person presses the button on the sales platform ([ADR-0002](docs/adr/0002-channel-strategy.md)).
- **Untrusted code is treated as untrusted.** Model-written CAD scripts pass an AST allow-list, run in a separate process with a cleared environment and a timeout, and never touch the file system themselves ([ADR-0003](docs/adr/0003-code-cad-build123d.md)).

## Status

Current version: **0.9.5** (Windows x64, macOS Apple silicon, macOS Intel). Research, Images, Modeling, Pricing and the project board are implemented and verified end to end in demo mode (scripted models, real CAD engine). Live provider APIs (DeepSeek, MiniMax, Qwen) have not yet been exercised with real keys. Installers are not code-signed yet, and there is no auto-update. Details: [implementation status](docs/w0-status.md), [release notes](docs/releases/).

## Technology

Rust · [Tauri 2](https://tauri.app) · [Leptos](https://leptos.dev) 0.7 (CSR, compiled to WebAssembly) · Tailwind CSS v4 · embedded SQLite (rusqlite) · three.js viewer · build123d / OpenCascade for CAD.

Supported providers: DeepSeek or any OpenAI-compatible endpoint (LLM and vision), Zhipu web search, MiniMax and Qwen (image generation and editing). A built-in **demo mode** exercises every workflow without any key.

## Documentation

The product and engineering documents are written in Chinese.

| Document | What it answers |
|----------|-----------------|
| [00 · Product brief](docs/00-product-brief.md) | What this is, for whom, why, and how success is measured |
| [01 · PRD](docs/01-prd.md) | Modules, priorities, acceptance criteria, open decisions |
| [02 · User journeys & information architecture](docs/02-ux-flows.md) | Main flow, page structure, wireframes, interaction rules |
| [03 · Architecture](docs/03-architecture.md) | Layers, interfaces, task system, agent orchestration, 3D pipeline, data model, packaging |
| [04 · Third-party integrations](docs/04-integrations.md) | Capabilities, prices and limits of the LLM, search, image, 3D, slicer and sales-channel providers |
| [05 · Growth & conversion](docs/05-growth-cro.md) | North-star metric, funnel, conversion levers, experiments, review cadence |
| [06 · Roadmap](docs/06-roadmap.md) | Phases, weekly deliverables, exit criteria |
| [07 · Risk & compliance](docs/07-risks-compliance.md) | Risk matrix, listing compliance checklist, what is deliberately not done |
| [08 · Market research](docs/08-market-research.md) | Market data, seller pain points, competitors and gaps (with sources) |
| [ADR-0001 · Technology stack](docs/adr/0001-tech-stack.md) | Why Rust + Tauri + Leptos |
| [ADR-0002 · Channel strategy](docs/adr/0002-channel-strategy.md) | Why semi-automatic publishing is the intended form, and where the red lines are |
| [ADR-0003 · Code CAD](docs/adr/0003-code-cad-build123d.md) | Why modeling is "image → build123d script", how quality and safety are achieved, engine distribution, measurements |
| [ADR-0004 · Modeling studio](docs/adr/0004-modeling-studio.md) | Designs, conversational editing, branching and undo, streaming and cancellation |
| [ADR-0005 · Image workbench](docs/adr/0005-image-studio.md) | Boards, the "current image", planner vs. image model, provider capabilities |
| [ADR-0006 · The project as the spine](docs/adr/0006-project-spine.md) | How the workbenches attach to a project and why stage gates are computed |
| [ADR-0007 · Cost & pricing](docs/adr/0007-cost-pricing.md) | The cost formula, suggested tiers, profit per machine hour, why profiles are copied rather than referenced |
| [ADR-0008 · Context menus](docs/adr/0008-context-menus.md) | Replacing the browser menu with context-sensitive application menus |

Suggested reading order: 00 → 01 → ADR-0001 / 0002 → 06; the rest as needed.

## Development

Requirements: Rust stable with the `wasm32-unknown-unknown` target, `trunk` 0.21 and `tauri-cli` 2.x. **Node.js is not required** — it is only used to rebuild the bundled three.js (`viewer3d/`), whose output is committed.

```
.\scripts\dev.ps1                 # development: trunk serve (17431) + the Tauri window + a WebView debugging port (17439)
cargo test --workspace            # all unit tests (front-end logic and i18n guards run on the host as well)
cargo check --target wasm32-unknown-unknown -p printpilot-ui   # check the front end only
cargo tauri build --bundles nsis  # build the Windows installer locally (target/release/bundle/nsis)
```

Code CAD needs the modeling engine on the development machine (one-off, about 600 MB, requires Python 3.10–3.13). It is installed into `%LOCALAPPDATA%\ai.printpilot\cad-engine\venv`:

```
.\scripts\setup-cad-engine.ps1
# Python tests for the sandbox runner and the prompt cheat sheet — run them with the engine's own interpreter:
& "$env:LOCALAPPDATA\ai.printpilot\cad-engine\venv\Scripts\python.exe" -X utf8 -m unittest discover -s crates\pp-cad\py
```

Without the engine everything else still works, and Rust tests that need it skip themselves. **End users never need Python**: release installers carry the engine pack (built by `scripts/build-engine-pack.py`).

To try the application without any API key, turn on **Settings → Demo mode**, create a design under *Modeling* and say one sentence: the built-in script runs the whole "spec → generate → conversational edit → parameter edit" flow on the real engine.

**Ports.** Tauri's default port 1420 and its neighbours are deliberately avoided so that several Tauri projects can run side by side: the development server uses **17431** and the WebView debugging port **17439**. `dev.ps1` probes both and moves to the next free port when one is taken, without touching files in the repository.

**Debugging the front end** (also works against a packaged build):

```
py scripts/cdp.py console 10      # replay the WebView console; also: targets / eval / watch / click / rclick / drag / type / shot
```

## Releasing

Pushing a `v*` tag makes GitHub Actions build three installers (Windows x64, macOS Apple silicon, macOS Intel) and publish a release once **all three** succeed:

```
# 1. bump the version in Cargo.toml, src-tauri/Cargo.toml and src-tauri/tauri.conf.json (a unit test keeps them in sync)
# 2. write docs/releases/vX.Y.Z.md (it becomes the release notes)
git commit -am "X.Y.Z" && git tag vX.Y.Z && git push origin main vX.Y.Z
```

Details, costs and what is still missing (code signing, notarisation, auto-update) are in `CLAUDE.md` and at the top of `.github/workflows/release.yml`.

## Repository layout

```
src/            front end (Leptos → wasm): app · state · controller · ui (component library) · view · ipc · viewer3d
src-tauri/      back-end shell: commands, the pp-asset:// protocol, configuration, bundling
crates/
  pp-common/    DTOs, enums and error codes shared by both sides; pure business rules (stage gates, cost engine)
  pp-db/        SQLite: schema, migrations, repositories
  pp-geometry/  mesh kernel: load, measure, flat-cut and cap, export STL / 3MF
  pp-providers/ provider adapters: LLM (DeepSeek / OpenAI-compatible, streaming), web search, image generation, mocks
  pp-agent/     bounded pipelines: market research, image planning, image → CAD; prompts live in prompts/
  pp-cad/       code CAD: engine discovery, sandboxed runner (py/runner.py), warm worker pool, parameter parsing, code contract
public/viewer3d/  three.js viewer bridge and the bundled three.js
locales/        interface strings (zh / en)
docs/           product and engineering documents, ADRs, release notes
```
