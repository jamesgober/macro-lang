# macro-lang - Roadmap

> Path from scaffold to a stable 1.0. Hard parts are front-loaded; each phase has hard exit criteria.
> Master plan: ../../_strategy/LANG_COLLECTION.md
>
> **Anti-deferral rule:** no listed hard task moves to a later phase unless this file records the move and the reason.

## v0.1.0 - Scaffold (DONE)
Compiles, CI green, structure correct, no domain logic.
- [x] Manifest, README, CHANGELOG, REPS, dual license, CI, deny, clippy, rustfmt.

## v0.2.0 - Core (THE HARD PART, NOT DEFERRED) (DONE)
Hygienic macro expansion: pattern matching plus template substitution over the AST.
Dependencies (wires ast, token, intern) are wired here, when first used.
Exit criteria:
- [x] Every public item has rustdoc + a runnable example.
- [x] Core invariants property-tested (full DIRECTIVES + API authored at this stage).

Delivered 2026-10-07: `Tree`, `Pattern` / `Fragment` / `Kleene`, `Template`,
`Rule`, `Macro`, `Expander`, `Context` / `Origin`, `MacroError`, `ExpandError`.
Matching is a backtracking-free NFA simulation with ambiguity detection;
hygiene is mark-based (a fresh context per expansion for template-introduced
tokens). Scaffold defects fixed on the way in: unquoted `keywords` /
`categories` in `Cargo.toml` (the manifest did not parse), clippy MSRV
`1.87` → `1.85`, BOM/CRLF in docs.

Dependency wiring, recorded under the anti-deferral rule:

- **`token-lang` — wired.** Token trees are built from its `Token<K>` and `Span`.
- **`intern-lang` — wired.** Macro and metavariable names are `Symbol`s.
- **`ast-lang` — not wired, by design.** Expansion runs on *token trees*,
  before parsing — the model of `macro_rules!` and Scheme `syntax-rules`, and
  the only one that lets a macro produce syntax the grammar then parses.
  ast-lang's `Node` trait exposes only a span and child handles, so a pattern
  could not name the construct it wants to match; matching "over the AST" would
  need a per-language node classifier and would tie expansion to one parse
  shape. The parser consumes the expanded trees, so the AST is downstream of
  this crate, not an input to it. This is a design decision, not a deferral.
- **`serde` — removed.** The scaffold declared it with no code behind it.
  Serializing `Tree<K>` (to cache expansions, for instance) can be added later
  as an additive feature.

## v1.0.0 - API freeze
Public surface stable and frozen until 2.0.
- [ ] docs/API.md marked stable; SemVer promise recorded.
- [ ] Full test + benchmark suite green on all three platforms.
