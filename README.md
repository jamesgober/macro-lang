<h1 align="center">
    <img width="99" alt="Rust logo" src="https://raw.githubusercontent.com/jamesgober/rust-collection/72baabd71f00e14aa9184efcb16fa3deddda3a0a/assets/rust-logo.svg">
    <br>
    <b>macro-lang</b>
    <br>
    <sub><sup>MACRO EXPANSION</sup></sub>
</h1>

<div align="center">
    <a href="https://crates.io/crates/macro-lang"><img alt="Crates.io" src="https://img.shields.io/crates/v/macro-lang"></a>
    <a href="https://crates.io/crates/macro-lang"><img alt="Downloads" src="https://img.shields.io/crates/d/macro-lang?color=%230099ff"></a>
    <a href="https://docs.rs/macro-lang"><img alt="docs.rs" src="https://img.shields.io/docsrs/macro-lang"></a>
    <a href="https://github.com/jamesgober/macro-lang/actions"><img alt="CI" src="https://github.com/jamesgober/macro-lang/actions/workflows/ci.yml/badge.svg"></a>
    <a href="https://github.com/rust-lang/rfcs/blob/master/text/2495-min-rust-version.md"><img alt="MSRV" src="https://img.shields.io/badge/MSRV-1.85%2B-blue"></a>
</div>

<br>

<div align="left">
    <p>
        <strong>macro-lang</strong> is a hygienic macro expander for language implementations: it matches an invocation's token trees against a macro's rules, writes out the template of the first rule that matches, and marks every token the template introduces so that a macro's internal names can never collide with its caller's. It is the <code>macro_rules!</code> and Scheme <code>syntax-rules</code> model, generic over your language's token kind.
    </p>
    <p>
        Macro expansion runs before parsing, and it is where many languages first get hygiene wrong. A macro that introduces a temporary called <code>tmp</code> breaks the moment a caller passes in a variable that is also called <code>tmp</code>. macro-lang solves that the way rustc does, with syntax contexts: each expansion mints a fresh context for the tokens its template writes, while the tokens it substitutes from the call site keep theirs. A name resolver that compares spelling <em>and</em> context then keeps the two apart without any renaming. The crate owns no syntax: you lex, you group tokens into trees, you lower your own macro-definition syntax into patterns and templates, and macro-lang does the matching, substitution, and hygiene bookkeeping.
    </p>
    <br>
    <hr>
    <p>
        <strong>MSRV is 1.85+</strong> (Rust 2024 edition). <code>no_std</code>-compatible (needs only <code>alloc</code>), <code>#![forbid(unsafe_code)]</code>, built on <a href="https://crates.io/crates/token-lang"><code>token-lang</code></a> and <a href="https://crates.io/crates/intern-lang"><code>intern-lang</code></a>.
    </p>
    <blockquote>
        <strong>1.0.0 is the API freeze.</strong> The public surface is stable and follows Semantic Versioning &mdash; no breaking changes before <code>2.0</code>. <strong>1.1.0</strong> adds, without breaking anything, an expansion <code>Budget</code> that is on by default, depth tracking that a self-rebuilding macro cannot escape, and separators that carry their definition context. See <a href="./docs/API.md#stability"><code>docs/API.md</code></a> for the frozen-surface list and the SemVer promise, and <a href="./CHANGELOG.md"><code>CHANGELOG.md</code></a>.
    </blockquote>
</div>

<hr>
<br>

## The model

A handful of plain-data types and one engine:

- A **[`Tree`](./docs/API.md#tree)** is a token tree: a single token, or a delimited group of trees. Invocations go in as trees and expansions come out as trees.
- A **[`Rule`](./docs/API.md#rule)** pairs a **[`Pattern`](./docs/API.md#pattern)** (what an invocation must look like) with a **[`Template`](./docs/API.md#template)** (what it expands to). Patterns bind *metavariables* — `$x:tt` — and repeat — `$( ... ),*`; templates substitute and repeat them.
- A **[`Macro`](./docs/API.md#macro)** is a name and an ordered list of rules, validated and compiled once when it is built.
- An **[`Expander`](./docs/API.md#expander)** runs expansions and keeps the hygiene record: every **[`Context`](./docs/API.md#context)** it mints, and the **[`Origin`](./docs/API.md#origin)** that says which expansion minted it.
- A **[`Budget`](./docs/API.md#budget)** bounds what an expander may do — nesting depth, number of expansions, tokens written — so no macro can hang the compiler or exhaust memory. It is on by default.

<br>

How an expansion treats each kind of token:

| Token in the output | Where it came from | Its context |
|---|---|---|
| **Literal** | Written by the template. | A context minted for this expansion. |
| **Substituted** | Captured from the invocation by a metavariable. | Unchanged — it belongs to the caller. |
| **Separator** | Written between template repetitions. | The expansion's minted context, marked from the separator's definition context (root for `Template::Repeat`, `ctx` for `Template::RepeatSeparated`). |

<hr>
<br>

## Installation

```toml
[dependencies]
macro-lang = "1"
token-lang = "1"    # Token, Span
intern-lang = "1"   # Symbol, Interner
```

Or from the terminal:

```bash
cargo add macro-lang token-lang intern-lang
```

MSRV: Rust 1.85 (Rust 2024 edition).

<hr>
<br>

## Quick start

A `list!` macro that turns `a, b, c` into `[a; b; c]`. Tokens here are plain `char`s to keep the example short; a real language uses its own token-kind enum.

```rust
use intern_lang::Interner;
use macro_lang::{Context, Expander, Fragment, Kleene, Macro, Pattern, Rule, Template, Tree};
use token_lang::{Span, Token};

let mut names = Interner::new();
let e = names.intern("e");
let s = Span::new(0, 0); // spans inside the macro definition

// list!( $($e:tt),* ) => [ $($e);* ]
let list = Macro::new(names.intern("list"), vec![Rule {
    pattern: vec![Pattern::Repeat {
        body: vec![Pattern::Bind { name: e, fragment: Fragment::Tree }],
        separator: Some(','),
        kleene: Kleene::ZeroOrMore,
    }],
    template: vec![Template::Group {
        open: Token::new('[', s),
        close: Token::new(']', s),
        body: vec![Template::Repeat {
            body: vec![Template::Var(e)],
            separator: Some(Token::new(';', s)),
        }],
    }],
}])?;

// The invocation's arguments, as token trees.
let input: Vec<Tree<char>> = "a,b,c"
    .char_indices()
    .map(|(i, c)| Tree::token(c, Span::new(i as u32, i as u32 + 1)))
    .collect();

let mut expander = Expander::new();
let out = expander.expand(&list, &input, Span::new(0, 13), Context::ROOT)?;

let Some(Tree::Group { trees, .. }) = out.first() else {
    return Err("expected one group".into());
};
let text: String = trees
    .iter()
    .filter_map(|t| match t {
        Tree::Token { token, .. } => Some(token.kind),
        Tree::Group { .. } => None,
    })
    .collect();
assert_eq!(text, "a;b;c");
# Ok::<(), Box<dyn std::error::Error>>(())
```

<br>

### Hygiene

The template below introduces a temporary `t`; the caller passes in a `t` of its own. After expansion both are spelled `t`, but they carry different contexts, so a resolver keyed on `(name, context)` sees two distinct variables.

```rust
use intern_lang::Interner;
use macro_lang::{Context, Expander, Fragment, Macro, Pattern, Rule, Template, Tree};
use token_lang::Span;

let mut names = Interner::new();
let x = names.intern("x");
let with_tmp = names.intern("with_tmp");

// with_tmp!($x:tt) => t = $x
let mac = Macro::new(with_tmp, vec![Rule {
    pattern: vec![Pattern::Bind { name: x, fragment: Fragment::Tree }],
    template: vec![
        Template::token('t', Span::new(100, 101)),
        Template::token('=', Span::new(102, 103)),
        Template::Var(x),
    ],
}])?;

let mut expander = Expander::new();
let out = expander.expand(&mac, &[Tree::token('t', Span::new(9, 10))], Span::new(0, 11), Context::ROOT)?;

let contexts: Vec<Context> = out
    .iter()
    .filter_map(|tree| match tree {
        Tree::Token { token, ctx } if token.kind == 't' => Some(*ctx),
        _ => None,
    })
    .collect();
let (macro_t, caller_t) = (contexts[0], contexts[1]);
assert_ne!(macro_t, caller_t);
assert!(caller_t.is_root());

// The expander can say where the macro's `t` came from.
let origin = expander.origin(macro_t).ok_or("not minted here")?;
assert_eq!(origin.macro_name, with_tmp);
assert_eq!(origin.call_site, Span::new(0, 11));
# Ok::<(), Box<dyn std::error::Error>>(())
```

### Bounded expansion

Every expander carries a [`Budget`](./docs/API.md#budget), applied automatically:

| Limit | Default | Stops | Error |
|---|---:|---|---|
| `max_depth` | 128 | A macro that calls itself without a base case. | `ExpandError::RecursionLimit` |
| `max_expansions` | 2<sup>20</sup> | A macro that fans out into exponentially many calls. | `ExpandError::Budget(Limit::Expansions { .. })` |
| `max_tokens` | 2<sup>22</sup> | A macro whose output grows every round, such as one that doubles its argument. | `ExpandError::Budget(Limit::Tokens { .. })` |

Expansion and token counts accumulate in the expander's `Usage` (`Expander::usage`). Tokens are charged before they are written, so an oversized expansion is refused without being built. Tighten or loosen the limits with `Expander::with_budget` / `set_budget`:

```rust
use intern_lang::Interner;
use macro_lang::{Budget, Context, ExpandError, Expander, Limit, Macro, Rule, Template};
use token_lang::Span;

let mut names = Interner::new();
let s = Span::new(0, 1);
// three!() => a b c
let three = Macro::new(names.intern("three"), vec![Rule {
    pattern: vec![],
    template: "abc".chars().map(|c| Template::token(c, s)).collect(),
}])?;

let mut expander = Expander::with_budget(Budget::DEFAULT.with_max_tokens(5));
assert!(expander.expand(&three, &[], s, Context::ROOT).is_ok());
assert_eq!(
    expander.expand(&three, &[], s, Context::ROOT),
    Err(ExpandError::Budget(Limit::Tokens { max: 5 })),
);
# Ok::<(), Box<dyn std::error::Error>>(())
```

**Long-lived hosts reset per unit of work.** A batch compiler uses one expander per compilation and never resets. A language server, REPL, or watch-mode compiler that keeps one expander calls `expander.reset_usage()` before each document parse, REPL entry, or rebuild, so the budget bounds each unit of work instead of the process, and one hostile document cannot use up the budget of the next. Contexts and origins survive the reset. See `examples/budget.rs`.

```rust
use intern_lang::Interner;
use macro_lang::{Budget, Context, Expander, Macro, Rule, Template};
use token_lang::Span;

let mut names = Interner::new();
let s = Span::new(0, 1);
let one = Macro::new(names.intern("one"), vec![Rule { pattern: vec![], template: vec![Template::token('k', s)] }])?;

let mut expander = Expander::with_budget(Budget::DEFAULT.with_max_expansions(100));
for _document in 0..1_000 {
    expander.reset_usage();                     // a fresh budget per document
    expander.expand(&one, &[], s, Context::ROOT)?;
}
assert_eq!(expander.usage().expansions, 1);     // 1 000 expansions, never over budget
# Ok::<(), Box<dyn std::error::Error>>(())
```

A driver that knows how deep each invocation sits should call [`Expander::expand_at`](./docs/API.md#expanderexpand_at) with that depth: the recursion limit then holds even for a macro that rebuilds its own invocation entirely from captured tokens, such as `m!($a:tt $b:tt) => $a $b ($a $b)` invoked as `m!(m !)`, which hygiene alone cannot tell apart from the original call. With plain `expand`, the budget stops that macro instead.

<hr>
<br>

## Examples

Four runnable examples ship in [`examples/`](./examples). They share a miniature front end in [`examples/common`](./examples/common/mod.rs): a lexer, a tree builder, a parser that lowers `macro_rules!`-style text such as `($a:ident, $b:ident) => { ... }` into patterns and templates, and an expansion driver. That module is the template for wiring macro-lang into a real language.

- **Hygiene** — the classic `swap!` macro, invoked on a variable that shares its temporary's name. The output prints contexts (`tmp#2` versus the caller's `tmp`), showing that the swap stays correct.
  ```bash
  cargo run --example hygiene
  ```
- **Vec literal** — a three-rule `vec!` that picks a rule by shape (`()`, `(x; n)`, `(a, b, c)`), expands a separated repetition, and renders a caret diagnostic from a failed match's span.
  ```bash
  cargo run --example vec_literal
  ```
- **Recursion** — a recursive `count!` macro expanded to a fixed point, a runaway macro stopped by the recursion limit, and an expansion backtrace built by following each context's origin outward.
  ```bash
  cargo run --example recursion
  ```
- **Budget** — three hostile macros stopped with no configuration: a self-rebuilding `quine!` under a depth-tracking driver (recursion limit) and under a rescanning driver (budget), and a doubling `dup!` (token budget); then a tight custom budget that still lets ordinary macros through, and a language-server loop that resets usage per document so a hostile document does not starve the next one.
  ```bash
  cargo run --example budget
  ```

<hr>
<br>

## Performance

Patterns are compiled once, when the macro is built, into a nondeterministic automaton; matching simulates it in a single left-to-right pass over the input, merging parses that reach the same point. There is no backtracking, so cost grows linearly with input length even for adversarial patterns. Rules that must start with a fixed token are rejected on the first input entry without running the automaton. The expander keeps every working buffer between calls, so steady-state expansion allocates only the output trees.

Measured with the benchmarks in [`benches/`](./benches) (x86_64, Rust stable, release profile, one reused expander):

| Benchmark | What it measures | Windows | Linux (WSL2) |
|---|---|---:|---:|
| `list/8` | `$($e:tt),* => [$($e);*]` over 8 elements. | ~0.56 µs | ~0.53 µs |
| `list/512` | The same over 512 elements (≈ 50 ns per element). | ~27 µs | ~26 µs |
| `nested/64` | `$( ( $($x:tt)* ) )*` over 64 groups of four tokens. | ~16 µs | ~13 µs |
| `dispatch/rules=16` | Sixteen rules keyed by a leading token, invoking the last. | ~0.13 µs | ~0.09 µs |
| `hygiene/literals=64` | A template writing 64 literal tokens, each marked. | ~0.56 µs | ~0.31 µs |
| `identity/depth=64` | `$($t:tt)*` capturing a 64-deep nested group. | ~4.7 µs | ~3.2 µs |
| `budget/dup/65536` | A doubling macro driven until a 2<sup>16</sup>-token budget stops it. | ~2.7 ms | not measured |
| `budget/quine/16384` | A self-rebuilding macro under `expand`, stopped by a 2<sup>14</sup>-expansion budget. | ~2.7 ms | not measured |
| `budget/limit=128` | The same macro under `expand_at`, stopped by the default recursion limit. | ~23 µs | not measured |

The rows above the budget rows were measured at 1.0.0. Budget accounting in 1.1.0 showed no measurable cost on the hot path: a min-of-60-batches A/B against 1.0.0 on Windows put `list/512` and `nested/64` within 2% of 1.0.0 (criterion runs on the shared build machine drifted by up to 8% between two runs of the *same* 1.0.0 binary, too noisy to resolve a few percent). The budget benchmarks scale linearly with the budget: `budget/dup` and `budget/quine` each take about 4× as long for a 4× larger budget.

Run them yourself:

```bash
cargo bench --bench bench
```

Criterion writes per-benchmark reports to `target/criterion/`. Numbers vary by CPU; use the trend across runs, not a single absolute.

<hr>
<br>

## Design notes

- **Token trees, not syntax trees.** Expansion runs before parsing. Grouping by delimiters is the one structure every language agrees on at that point, and it is exactly what a pattern needs: a group matches as a unit, and a metavariable can capture a whole bracketed expression without knowing the grammar inside.
- **Hygiene by marking, not renaming.** No identifier is rewritten. Each expansion mints a context for the tokens its template introduces, and `Expander::origin` records where every context came from — the definition-site context to fall back on for free names, the macro, and the call site for diagnostics.
- **Errors at definition time when possible.** `Macro::new` rejects duplicate bindings, unbound or under-repeated metavariables, repetitions that could match nothing, and template repetitions with nothing to repeat over. A macro that builds can only fail on input-dependent conditions.
- **Ambiguity is an error.** When a rule could match an invocation in two different ways, expansion reports `ExpandError::Ambiguous` rather than guessing. Unlike rustc, the matcher still accepts patterns such as `$($t:tt)* ;` whose apparent ambiguity is resolved by the input that follows.
- **Deep input is safe.** Flattening, matching, transcription, and rebuilding captured groups all run on explicit buffers rather than recursion, so a deeply nested invocation cannot overflow the stack inside the expander.
- **Runaway expansion is bounded.** An invocation's depth is the deepest of the expansions that produced its name token or any input token (and, through `expand_at`, the depth the driver tracked), so a macro that expands into itself without a base case stops with `ExpandError::RecursionLimit` (128 nested expansions by default). A default `Budget` caps total expansions and tokens, which stops doubling and fan-out bombs that never nest deeply.

<hr>
<br>

## Testing

The suite runs on Windows, Linux (WSL2 Ubuntu), and macOS through the CI matrix, on stable and the 1.85 MSRV:

```bash
cargo test                       # unit + integration + property + doctests
cargo clippy --all-targets --all-features -- -D warnings
cargo bench --bench bench
```

The property tests in [`tests/proptests.rs`](./tests/proptests.rs) hold the matcher to an independent oracle: for arbitrary patterns and inputs, a brute-force counter of every distinct parse must agree with the expander. No parse means `NoMatch`, exactly one means success, and more than one means `Ambiguous`. On every unique match, a template that mirrors the pattern must reproduce the input token for token. Every `rust` example in this README and in [`docs/API.md`](./docs/API.md) is compiled and run as a doctest, so the published examples cannot drift from the API.

<hr>
<br>

## Cross-platform support

- Linux (x86_64, aarch64)
- macOS (x86_64, Apple Silicon)
- Windows (x86_64)

The expander uses no operating-system facilities and no platform-specific code; behaviour is identical on every target.

<hr>
<br>

## Contributing

See [`REPS.md`](./REPS.md) for the engineering standards every change is held to, and [`dev/ROADMAP.md`](./dev/ROADMAP.md) for the plan to 1.0. Before a PR: `cargo fmt --all`, `cargo clippy --all-targets --all-features -- -D warnings`, and `cargo test --all-features` must be clean.

<br>

<div id="license">
    <h2>License</h2>
    <p>Licensed under either of</p>
    <ul>
        <li><b>Apache License, Version 2.0</b> &mdash; <a href="./LICENSE-APACHE">LICENSE-APACHE</a></li>
        <li><b>MIT License</b> &mdash; <a href="./LICENSE-MIT">LICENSE-MIT</a></li>
    </ul>
    <p>at your option.</p>
</div>

<div align="center">
  <h2></h2>
  <sup>COPYRIGHT <small>&copy;</small> 2026 <strong>James Gober <me@jamesgober.com>.</strong></sup>
</div>
