# Agent instructions

Every agent working in this repository follows the working rules in [CLAUDE.md](CLAUDE.md),
whatever its model or runtime (Claude Code, Codex, Marten or anything else). Read all of it before
you change anything. CLAUDE.md is the single source of truth; this file only points at it, so
that runtimes which look for `AGENTS.md` find the same rules.

Background for each workstream is in [docs/handoff/](docs/handoff/HANDOFF.md)
(`docs/handoff/streams/<stream>.md`). It reflects the state on 2026-11-15; your brief and the
project's memory are newer where they differ. Triage tooling is described in
[tools/triage/README.md](tools/triage/README.md), and the open gaps in
[docs/KNOWN_GAPS.md](docs/KNOWN_GAPS.md).
