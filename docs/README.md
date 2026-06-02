# ALVR documentation

This is the documentation hub for the ALVR workspace. Project-level design docs live
here; tooling, contribution, and per-crate docs stay next to what they describe (see
[Other doc locations](#other-doc-locations)).

## Project map

- [`ARCHITECTURE.md`](ARCHITECTURE.md) — the whole-system map: crate/module boundaries,
  the streamer↔client data flows, and the named runtime threads. Read this before
  refactoring data structures or thread-ownership boundaries (CLAUDE.md rule 4).

## OpenXR runtime mode (preview)

ALVR can run its PC streamer either as a SteamVR driver (default) or as a Monado-based
OpenXR runtime. The OpenXR-mode work is documented as:

- [`openxr-migration.md`](openxr-migration.md) — the **master plan**: phase breakdown,
  risk list, and decisions. Authoritative when it disagrees with the pickup doc.
- [`monado-notes/`](monado-notes/) — ALVR-side reference notes on the Monado tree
  (`openxr/`) plus the live pickup doc. Start at [`monado-notes/README.md`](monado-notes/README.md):
  - [`monado-notes/NEXT_STEPS.md`](monado-notes/NEXT_STEPS.md) — **canonical pickup doc**
    for future sessions (current status, what's left).
  - Reference: `STRUCTURE`, `XRT_INTERFACES`, `DATAFLOW`, `COMPOSITOR`, `IPC`, `DRIVERS`,
    `STATE_TRACKERS`, `TARGETS`, and `ARCHITECTURE` (the *Monado-tree* map — distinct from
    the whole-ALVR `ARCHITECTURE.md` above).
  - Integration/feature notes: `INTEGRATION_NOTES`, `HAND_TRACKING_PASSTHROUGH`,
    `PER_VIEW_FOVEATION`, `SMOKE_TESTS`, `SUBMODULE_PIN`.
  - Working scope docs: `PHASE2_MANIFEST`, `PHASE3_0_SCOPE`, `PHASE7_SLICE2_SCOPE`.
  - [`monado-notes/archive/`](monado-notes/archive/) — historical session logs, kept for
    provenance (not current reference).

## Other doc locations

These live with the thing they document rather than here:

- **Repo root** — `README.md` (project landing), `CONTRIBUTING.md`, `CHANGELOG.md`,
  `CLAUDE.md` (agent instructions; auto-loaded from root).
- **Per-crate** — `alvr/*/README.md`, `metrics/README.md` (rendered per-directory on GitHub).
- **User guides** — [`wiki/`](../wiki/) (GitHub-wiki content: installation, troubleshooting,
  settings, FFR, …).
- **Tooling** — `.claude/agents/`, `.claude/commands/`, `.claude/skills/` (discovered by
  path by Claude Code), `.github/ISSUE_TEMPLATE/`.
