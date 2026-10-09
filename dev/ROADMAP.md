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

## v1.0.0 - API freeze (DONE)
Public surface stable and frozen until 2.0.
- [x] docs/API.md marked stable; SemVer promise recorded.
- [x] Full test + benchmark suite green on all three platforms.

Shipped 2026-10-07. Before freezing, `Pattern`, `Fragment`, `Template`, and
`Origin` were made `#[non_exhaustive]` so the definition language and the
hygiene record can grow additively in 1.x (`Tree`, `Rule`, and `Kleene` stay
exhaustive: users match on trees, build rules with struct literals, and the
three Kleene operators are a closed set). The surface, the matching and hygiene
semantics, the definition checks, the default recursion limit, and MSRV 1.85
are recorded as the contract in `docs/API.md#stability`. Tests and benchmarks
green on Windows and Linux (WSL2) locally; macOS through the CI matrix.

## v1.1.0 - Termination and resource bounds (DONE)
Additive minor release fixing ISSUES H07, H08, and the patch part of M70.
Exit criteria:
- [x] The recursion limit holds for every macro that rebuilds its own
      invocation, given a driver that tracks depth (H07).
- [x] A default budget stops doubling and fan-out macros in bounded time and
      memory, with no code change for existing users (H08).
- [x] Repetition separators can be marked from their definition context (M70,
      patch part).
- [x] Regression tests for every reported shape, property tests for the new
      invariants, a benchmark showing bounded cost; gate green on Windows.

Delivered 2026-10-08:

- **`Budget { max_depth, max_expansions, max_tokens }`** (`#[non_exhaustive]`)
  with `Budget::DEFAULT` (128, `2^20`, `2^22`), `Default`, and `const`
  `with_*` builders; **`Limit`** and **`ExpandError::Budget(Limit)`**
  (`ExpandError` was already `#[non_exhaustive]`, so the variant is additive);
  **`Expander::with_budget` / `budget` / `set_budget`**. `Expander::new` and
  `with_limit` apply the default budget, so protection needs no code change.
  **`Usage`, `Expander::usage`, `Expander::reset_usage`**: counts accumulate
  until a reset, which zeroes only the counters (hygiene, budget, and depth
  untouched), so a long-lived host (LSP, REPL, watch mode) budgets per unit of
  work. Added at review, replacing the lifetime-only caps of the first draft;
  tokens are charged before they are written, captures by their flattened
  size, so an oversized expansion is refused without being copied; failures
  are not charged.
- **Depth independent of token marks.** `Expander::expand_at(.., depth)` lets
  the driver say how deep the code it found the invocation in is; the
  invocation runs one level below the deepest of that depth, the expansion
  that produced `call_context`, and the expansions that produced any input
  token (collected while flattening, no extra pass). `expand` is
  `expand_at(.., 0)`.
- **`Template::RepeatSeparated { body, separator, ctx }`**: the separator is
  marked from `ctx`, sharing the minted context of the definition's other
  literals. `Template::Repeat` keeps its documented root-context separator.
- `examples/budget.rs`; `Driver::expand_tracked` in `examples/common`, whose
  definition lowering now keeps separator contexts; `tests/budget.rs`
  (34 tests including 3 property tests); `budget/*` benchmarks.

Limits of the fix, recorded rather than hidden:

- **Plain `expand` cannot see the depth of an invocation built only from
  captured source tokens.** `m!($a:tt $b:tt) => $a $b ($a $b)` invoked as
  `m!(m !)` produces a call that is token-for-token identical to the original
  (captured tokens keep `ROOT`, and `Tree::Group` delimiters carry no context
  — `Tree` is exhaustive and frozen, so a context cannot be added to them in
  1.x). The two calls are indistinguishable to the expander, so under a
  rescanning driver that uses `expand` this macro is stopped by the expansion
  or token budget (`ExpandError::Budget`), not by `RecursionLimit`. A driver
  that tracks depth and calls `expand_at` gets `RecursionLimit` for every
  shape. Giving `Tree::Group` a context is a 2.0 candidate.
- **`Template::Repeat` separators stay root-defined.** Its `separator` field is
  a bare `Token` with no context, and the variant is frozen; inferring a
  context from neighbouring literals would be a guess. Front ends lowering
  macro-generated definitions should emit `RepeatSeparated`.
- Performance: a min-of-batches A/B against 1.0.0 on Windows shows the hot
  path within 2% (`list/512`, `nested/64`). Criterion A/B runs on the shared
  build machine were inconclusive (the same 1.0.0 binary drifted up to 8%
  between runs), so a quiet-machine criterion comparison is still owed.

Dependency wiring: unchanged — `token-lang = "1"` and `intern-lang = "1"`; no
new dependencies.

Moved out of this release, recorded under the anti-deferral rule (ISSUES M70,
Phase 2/3 parts):

- **CST adapter** — expanding over `syntax-lang` trees rather than token-lang
  `Tree`s. It needs a stable mapping between the two tree shapes and a
  decision on how hygiene contexts ride on CST tokens; that is new design
  work, not a patch, and it is not in a bug-fix minor.
- **Parser-backed fragments** — `$e:expr`-style fragments that call a
  language's parser to decide how much input a binding takes. It needs a
  parser callback in `Fragment` (additive: `Fragment` is
  `#[non_exhaustive]`) and a matcher change, because today's matcher
  consumes exactly one tree per binding. Planned with the CST adapter.

Additive 1.x candidates (not commitments): the CST adapter and parser-backed
fragments above, a `serde` feature for `Tree`, metavariable expressions in
templates (`${count(x)}`-style), and further `Fragment` kinds.
