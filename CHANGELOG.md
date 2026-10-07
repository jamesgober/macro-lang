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

[Unreleased]: https://github.com/jamesgober/macro-lang/compare/v1.0.0...HEAD
[1.0.0]: https://github.com/jamesgober/macro-lang/compare/v0.2.0...v1.0.0
[0.2.0]: https://github.com/jamesgober/macro-lang/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/jamesgober/macro-lang/releases/tag/v0.1.0
