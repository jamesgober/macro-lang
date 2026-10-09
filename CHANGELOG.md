<h1 align="center">
    <img width="90px" height="auto" src="https://raw.githubusercontent.com/jamesgober/jamesgober/main/media/icons/hexagon-3.svg" alt="Triple Hexagon">
    <br><b>CHANGELOG</b>
</h1>
<p>
  All notable changes to <code>macro-lang</code> will be documented in this file. The format is based on <a href="https://keepachangelog.com/en/1.1.0/">Keep a Changelog</a>,
  and this project adheres to <a href="https://semver.org/spec/v2.0.0.html/">Semantic Versioning</a>.
</p>

---

## [Unreleased]

### Added

### Changed

### Fixed

### Security

---

## [1.1.0] - 2026-10-08

Termination and resource bounds. Every expander now enforces a budget, on by
default, so no macro can hang a compiler or exhaust its memory; the recursion
limit can no longer be escaped by a macro that rebuilds its own invocation; and
repetition separators are marked from their definition context like every
other template literal. All API changes are additive: code that builds against
1.0 builds unchanged.

### Added

- `Budget { max_depth, max_expansions, max_tokens }` (`#[non_exhaustive]`), with
  `Budget::DEFAULT` (depth 128, `2^20` expansions, `2^22` tokens),
  `Default`, and the `const` builders `with_max_depth`, `with_max_expansions`,
  and `with_max_tokens`. Expansion and token counts accumulate until
  `Expander::reset_usage`; tokens are counted one per token, two per group, plus every token
  of a substituted capture, and are charged *before* they are written, so an
  oversized expansion is refused without being built.
- `Limit` (`#[non_exhaustive]`: `Expansions { max }`, `Tokens { max }`) and
  `ExpandError::Budget(Limit)`, reported when a budget runs out. A failed
  expansion is not charged and leaves the expander unchanged, as before.
- `Usage { expansions, tokens }` (`#[non_exhaustive]`, `Usage::NONE`),
  `Expander::usage()`, and `Expander::reset_usage()`, which zeroes the counters
  and leaves the budget and all hygiene state untouched. A long-lived host (a
  language server, a REPL, a watch-mode compiler) calls it once per unit of
  work so the budget bounds each document, entry, or rebuild rather than the
  process.
- `Expander::with_budget`, `Expander::budget`, and `Expander::set_budget`.
  `Expander::new` and `Expander::with_limit` apply `Budget::DEFAULT`
  (with the given depth), so existing users are protected without code changes.
- `Expander::expand_at(mac, input, call_site, call_context, depth)`: `expand`
  for a driver that knows how deep in nested expansion it found the invocation.
  With it the recursion limit bounds every chain of nested expansions,
  whatever tokens the macros reuse.
- `Template::RepeatSeparated { body, separator, ctx }`: a separated repetition
  whose separator carries its definition context, for front ends lowering a
  macro definition that an earlier expansion produced.
- `examples/budget.rs` (hostile macros stopped by the defaults), a depth-tracking
  driver `Driver::expand_tracked` in `examples/common`, `tests/budget.rs`
  (regressions for every reported shape plus three property tests), and the
  `budget/*` benchmarks showing that the cost of an expansion bomb is linear in
  the budget.

### Changed

- An invocation's nesting depth is now one more than the deepest of: the
  driver's `depth` (`0` through `expand`), the expansion that produced
  `call_context`, and the expansions that produced **any token of its input**.
  1.0 read `call_context` alone. Depth can only be higher than 1.0 computed,
  never lower; for ordinary recursive macros it is unchanged.
- The steady-state benchmarks run on an expander with the expansion and token caps lifted,
  since a benchmark loop performs far more expansions than a compilation.
  Budget accounting showed no measurable hot-path cost (min-of-batches A/B
  against 1.0.0 on Windows: within 2%); a repetition's separator context is
  now looked up once per repetition instead of once per separator.
- `examples/common` lowers repetition separators with their contexts
  (`Template::RepeatSeparated`).

### Fixed

- Repetition separators can now be marked from their definition context. With
  `Template::RepeatSeparated`, a separator shares the context the expansion
  mints for the other literals of its definition and `Origin::parent` reports
  that definition context; 1.0 always marked separators as if defined in the
  root context, which split a macro-defined macro's separators off from its
  other literals. `Template::Repeat` keeps its documented root-context
  separator (exact for macros written in source).

### Security

- **Recursion-limit bypass (H07).** A macro that rebuilt its own invocation
  from captured tokens (for example `m!($a:tt $b:tt) => $a $b ($a $b)` invoked
  as `m!(m !)`) restarted at depth 1 every round and looped forever. Depth now
  also follows the input tokens, `expand_at` lets a driver track depth
  independently of hygiene (closing the case where every token is captured
  from source), and the default budget bounds the remaining case under plain
  `expand`.
- **Expansion bombs (H08).** Nothing bounded total output: a doubling macro
  reached `2^depth` tokens, and a fan-out macro `2^depth` expansions, within
  the recursion limit. The default `Budget` stops both in bounded time and
  memory (the doubling macro in `examples/budget`: about 0.2 s and a 400 MB
  whole-process peak in a release build).

---

## [1.0.0] - 2026-10-07

API freeze. The public surface introduced in 0.2.0 is now stable and frozen
under Semantic Versioning: no breaking changes ship before `2.0`. The only code
change reserves room for additive growth in the definition types before the
freeze makes that impossible.

### Changed

- Bumped the crate version to `1.0.0` and declared the public API stable.
- `Pattern`, `Fragment`, and `Template` are now `#[non_exhaustive]`, so new
  kinds of pattern element, fragment, and template element can be added in a
  `1.x` release. Constructing any variant is unchanged; a downstream `match` on
  these enums now needs a wildcard arm.
- `Origin` is now `#[non_exhaustive]`, so fields can be added in a `1.x`
  release. Reading its fields is unchanged; destructuring needs a trailing `..`.
- `docs/API.md` marked stable with a recorded SemVer promise: the frozen
  surface, the matching semantics (rule order, whole-input matching, kind-only
  literal comparison, ambiguity as an error), the hygiene semantics (fresh
  contexts for template literals, untouched captures, rollback on failure),
  the fixed set of definition checks, the default recursion limit, and MSRV
  1.85 as a compatibility surface.
- Crate-level documentation gained a Stability section; the README and
  `docs/API.md` install snippets now use `macro-lang = "1"`, and the README
  performance table reports Windows and Linux side by side.

---

## [0.2.0] - 2026-10-07

The core, and the hard part of the roadmap: the scaffold becomes a working
hygienic macro expander. Macros are lists of pattern/template rules over token
trees, in the `macro_rules!` model; matching is a backtracking-free automaton
simulation that detects ambiguity instead of guessing; and every token a
template introduces receives a hygiene context minted for its expansion. The
surface is small and data-oriented, and generic over the language's token kind.

### Added

- `Tree<K>` — a token tree: a `token_lang::Token<K>` with its hygiene context, or
  a delimited group. The input and output of every expansion. `Tree::token`
  builds a root-context token; `Tree::span` reports the covered source range.
- `Pattern<K>`, `Fragment<K>`, `Kleene` — the match side of a rule: literal
  tokens, delimited groups, metavariable bindings (`$x:tt`, or a token-kind test
  for specifiers like `ident`), and repetitions (`*`, `+`, `?`, with optional
  separators).
- `Template<K>` — the output side of a rule: literal tokens and groups,
  metavariable substitution, and repetitions expanded in lockstep.
  `Template::token` builds a root-context literal.
- `Rule<K>` and `Macro<K>` — a macro is a name and an ordered list of rules,
  validated and compiled once by `Macro::new`. Every check that does not depend
  on an invocation is made there.
- `Expander<K>` — runs expansions (`expand`), owns the hygiene table
  (`origin`), and pools its working buffers so steady-state expansion allocates
  only the output. A recursion limit (`with_limit`, default 128) stops runaway
  recursive macros.
- `Context` and `Origin` — hygiene contexts, and the record of which expansion
  minted each one: the definition-site context for resolver fallback, the macro
  name, the call site, and the caller's context for backtraces.
- `MacroError` and `ExpandError` — `#[non_exhaustive]` errors for definitions
  (duplicate or unbound metavariables, wrong repetition depth, repetitions that
  could match nothing) and invocations (no match with the offending token's
  span, ambiguity, lockstep mismatch, recursion limit, context exhaustion).
- Three runnable examples (`hygiene`, `vec_literal`, `recursion`) sharing a
  miniature language front end that lowers `macro_rules!`-style text into
  rules; Criterion benchmarks; integration tests; property tests that check the
  matcher against an independent brute-force parse counter; and full
  `docs/API.md`.

### Changed

- Bumped the crate version to `0.2.0`.
- Wired `token-lang` 1 (token trees are built from its `Token` and `Span`) and
  `intern-lang` 1 (macro and metavariable names are `Symbol`s). `ast-lang` is
  deliberately not wired: expansion runs on token trees before parsing, and the
  reasoning is recorded in `dev/ROADMAP.md`.
- Removed the unused `serde` feature and dependency and the unused `loom`
  dev-dependency from the scaffold. Neither had any code behind it; both can be
  added back additively if a use appears.
- Applied the full REPS crate-root lint set in `src/lib.rs`.

### Fixed

- Corrected invalid TOML in `Cargo.toml`: `keywords` and `categories` were
  unquoted, which stopped the manifest from parsing at all.
- Aligned the `clippy.toml` MSRV (`1.87` → `1.85`) with `Cargo.toml`.
- Removed UTF-8 byte-order marks and CRLF line endings from `docs/API.md` and
  `dev/ROADMAP.md`, and a stale project name in `deny.toml`.
- README contributing guidance now points at `REPS.md`; it previously linked a
  `dev/DIRECTIVES.md` that does not exist.

---

## [0.1.0] - 2026-06-18

Initial scaffold and repository bootstrap. No domain logic yet &mdash; this release establishes the structure, tooling, and quality gates the implementation will be built on.

### Added

- `Cargo.toml` with crate metadata, Rust 2024 edition, MSRV 1.85.
- Dual `Apache-2.0 OR MIT` license files.
- `README.md`, `CHANGELOG.md`, and a documentation skeleton.
- `REPS.md` compliance baseline.
- `.github/workflows/ci.yml` CI matrix; `deny.toml`, `clippy.toml`, `rustfmt.toml`.
- `dev/DIRECTIVES.md` and `dev/ROADMAP.md` (committed engineering standards + plan).

[Unreleased]: https://github.com/jamesgober/macro-lang/compare/v1.1.0...HEAD
[1.1.0]: https://github.com/jamesgober/macro-lang/compare/v1.0.0...v1.1.0
[1.0.0]: https://github.com/jamesgober/macro-lang/compare/v0.2.0...v1.0.0
[0.2.0]: https://github.com/jamesgober/macro-lang/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/jamesgober/macro-lang/releases/tag/v0.1.0
