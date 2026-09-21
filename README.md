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
- **Publishing packs** — AI drafts notes (by angle) and the shop listing, a compliance check runs as you type (length limits, superlatives, health claims, brand names, off-platform contact, real photo first), and everything is packed into a folder of processed images and copy. Publish from the computer (official page + step-by-step copy) or from a phone via a temporary read-only LAN page. The application never posts on your behalf.
- **Native-feeling UI** — context menus for every object, streaming AI replies, and any running turn can be cancelled.
