# macro-lang &mdash; API Reference

> Complete reference for every public item in `macro-lang`, with examples.
> **Status: stable (1.x).** The surface below is the `1.0` contract plus the
> additive `1.1` items (marked *Added in 1.1.0*); it follows
> [Semantic Versioning](#stability) and will not change in a breaking way before
> `2.0`. See [`../dev/ROADMAP.md`](../dev/ROADMAP.md).

<sub>Copyright &copy; 2026 <strong>James Gober</strong>.</sub>

## Table of contents

- [Overview](#overview)
- [Installation](#installation)
- [Quick start](#quick-start)
- [Concepts](#concepts)
  - [Token trees](#token-trees)
  - [Rules, patterns, and templates](#rules-patterns-and-templates)
  - [How matching works](#how-matching-works)
  - [Repetition depth and lockstep](#repetition-depth-and-lockstep)
  - [Hygiene](#hygiene)
  - [Termination and budgets](#termination-and-budgets)
- [`Tree`](#tree)
  - [`Tree::token`](#treetoken)
  - [`Tree::span`](#treespan)
- [`Pattern`](#pattern)
- [`Fragment`](#fragment)
- [`Kleene`](#kleene)
  - [`Kleene::allows`](#kleeneallows)
- [`Template`](#template)
  - [`Template::token`](#templatetoken)
- [`Rule`](#rule)
- [`Macro`](#macro)
  - [`Macro::new`](#macronew)
  - [`Macro::name`](#macroname)
- [`Expander`](#expander)
  - [`Expander::DEFAULT_LIMIT`](#expanderdefault_limit)
  - [`Expander::new`](#expandernew)
  - [`Expander::with_limit`](#expanderwith_limit)
  - [`Expander::limit`](#expanderlimit)
  - [`Expander::with_budget`](#expanderwith_budget)
  - [`Expander::budget`](#expanderbudget)
  - [`Expander::set_budget`](#expanderset_budget)
  - [`Expander::usage`](#expanderusage)
  - [`Expander::reset_usage`](#expanderreset_usage)
  - [`Expander::expand`](#expanderexpand)
  - [`Expander::expand_at`](#expanderexpand_at)
  - [`Expander::origin`](#expanderorigin)
- [`Budget`](#budget)
  - [`Budget::DEFAULT`](#budgetdefault)
  - [`Budget::with_max_depth`, `with_max_expansions`, `with_max_tokens`](#budgetwith_max_depth-with_max_expansions-with_max_tokens)
- [`Limit`](#limit)
- [`Usage`](#usage)
- [`Context`](#context)
- [`Origin`](#origin)
- [`MacroError`](#macroerror)
- [`ExpandError`](#expanderror)
- [Feature flags](#feature-flags)
- [Guide: writing a front end](#guide-writing-a-front-end)
- [Guide: resolving names with contexts](#guide-resolving-names-with-contexts)
- [Guide: bounding untrusted macros](#guide-bounding-untrusted-macros)
- [Stability](#stability)

---

## Overview

macro-lang is a hygienic macro expander for language implementations — the
`macro_rules!` / Scheme `syntax-rules` model, generic over a language's token
kind. It matches an invocation's token trees against a macro's rules, writes out
the template of the first rule that matches, and gives every token the template
introduces a fresh hygiene context.

| Item | Role |
|---|---|
| [`Tree`](#tree) | A token tree: one token, or a delimited group. Input and output of every expansion. |
| [`Pattern`](#pattern) | One element of a rule's match side. |
| [`Fragment`](#fragment) | What a metavariable binding may capture. |
| [`Kleene`](#kleene) | How many times a pattern repetition may match. |
| [`Template`](#template) | One element of a rule's output side. |
| [`Rule`](#rule) | A pattern paired with a template. |
| [`Macro`](#macro) | A validated, compiled list of rules under a name. |
| [`Expander`](#expander) | Runs expansions; owns the hygiene table, the budget, and the scratch buffers. |
| [`Budget`](#budget) | Limits on nesting depth, expansions, and tokens written. |
| [`Limit`](#limit) | Which part of a budget an expansion ran out of. |
| [`Usage`](#usage) | The work charged against a budget since the last reset. |
| [`Context`](#context) | The hygiene context of a token. |
| [`Origin`](#origin) | Where a minted context came from. |
| [`MacroError`](#macroerror) | Why a definition was rejected. |
| [`ExpandError`](#expanderror) | Why an invocation could not be expanded. |

The crate is `#![forbid(unsafe_code)]`, `no_std`-compatible (needs only
`alloc`), and depends on [`token-lang`](https://crates.io/crates/token-lang)
(`Token`, `Span`) and [`intern-lang`](https://crates.io/crates/intern-lang)
(`Symbol`).

---

## Installation

```toml
[dependencies]
macro-lang = "1"
token-lang = "1"
intern-lang = "1"
```

Or from the terminal:

```bash
cargo add macro-lang token-lang intern-lang
```

MSRV: Rust 1.85 (Rust 2024 edition).

---

## Quick start

`twice!($x:tt) => $x $x`, built and expanded. The token kind here is `char`;
a real language uses its own kind enum.

```rust
use intern_lang::Interner;
use macro_lang::{Context, Expander, Fragment, Macro, Pattern, Rule, Template, Tree};
use token_lang::Span;

let mut names = Interner::new();
let x = names.intern("x");

let twice = Macro::new(names.intern("twice"), vec![Rule {
    pattern: vec![Pattern::Bind { name: x, fragment: Fragment::Tree }],
    template: vec![Template::Var(x), Template::Var(x)],
}])?;

let mut expander = Expander::new();
let input = [Tree::token('a', Span::new(7, 8))];
let out = expander.expand(&twice, &input, Span::new(0, 9), Context::ROOT)?;
assert_eq!(out, vec![input[0].clone(), input[0].clone()]);
# Ok::<(), Box<dyn std::error::Error>>(())
```

---

## Concepts

### Token trees

Expansion works on **token trees**: each tree is a single token or a delimited
group of trees. Grouping by delimiters is the only structure every language has
before parsing, and it is the structure patterns need — a group is matched as a
unit, and a metavariable can capture an entire bracketed expression without
knowing the grammar inside it. Expansion runs before parsing; the parser consumes
the expanded trees.

Tokens are [`token_lang::Token<K>`](https://docs.rs/token-lang): a kind `K` of
your choosing and a source span. macro-lang only clones kinds and compares them
for equality, so a small `Copy` kind — an enum whose identifiers and literals
carry interned `Symbol`s — is the fast path.

### Rules, patterns, and templates

A [`Macro`](#macro) is an ordered list of [`Rule`](#rule)s. Each rule has a
**pattern**, matched against the invocation, and a **template**, written out when
the pattern matches.

| `macro_rules!` syntax | Pattern element | Template element |
|---|---|---|
| a literal token `else` | `Pattern::Token(kind)` | `Template::Token { token, ctx }` |
| a group `( ... )` | `Pattern::Group { open, close, body }` | `Template::Group { open, close, body }` |
| `$x:tt` / `$x` | `Pattern::Bind { name, fragment }` | `Template::Var(name)` |
| `$( ... ),*` | `Pattern::Repeat { body, separator, kleene }` | `Template::Repeat { body, separator }` |

Patterns and templates are plain data. A language's front end parses its own
macro-definition syntax and lowers it into these elements (see
[writing a front end](#guide-writing-a-front-end)).

### How matching works

- Rules are tried **in order**; the first rule whose pattern matches the
  **entire** invocation is used.
- Literal tokens compare by **kind only** — spans and hygiene contexts never
  affect a match.
- A pattern is compiled into a nondeterministic automaton and simulated in one
  left-to-right pass, merging parses that reach the same point. There is no
  backtracking: cost grows linearly with the input even for adversarial
  patterns.
- If a rule matches in **more than one way**, expansion stops with
  [`ExpandError::Ambiguous`](#expanderror) rather than guessing. An apparent
  ambiguity that the rest of the input resolves — `$($t:tt)* ;` followed by
  input ending in `;` — is not an error.
- If no rule matches, [`ExpandError::NoMatch`](#expanderror) points at the token
  the furthest-reaching rule could not get past.

### Repetition depth and lockstep

A metavariable bound inside `n` pattern repetitions has **depth** `n` and
captures one tree per repetition. In the template:

- it must be used inside at least `n` template repetitions
  ([`MacroError::StillRepeating`](#macroerror) otherwise);
- used inside more than `n`, it is repeated as a whole across the extra levels;
- every template repetition needs at least one metavariable that repeats at its
  depth, which decides the iteration count
  ([`MacroError::NoRepeatingVariable`](#macroerror) otherwise);
- all metavariables that repeat at a template repetition's depth advance in
  **lockstep** and must have captured the same number of trees
  ([`ExpandError::RepetitionMismatch`](#expanderror) otherwise).

### Hygiene

Every token in a tree carries a [`Context`](#context). Source tokens carry
[`Context::ROOT`](#context). Each expansion mints a context for the **literal**
tokens its template writes, while tokens **substituted** from the invocation keep
the context they arrived with. A name resolver that compares spelling *and*
context keeps a macro's internal names and its caller's names apart.
[`Expander::origin`](#expanderorigin) reports, for any minted context, the
context the token had in the macro definition (for definition-site fallback),
the macro, and the call site. See
[resolving names with contexts](#guide-resolving-names-with-contexts).

### Termination and budgets

Macros are untrusted input: a two-line macro can describe unbounded work. Every
[`Expander`](#expander) therefore enforces a [`Budget`](#budget), on by default:

- **Depth** (`max_depth`, default 128). An invocation in source runs at depth 1;
  an invocation runs one level deeper than the deepest expansion that produced
  its name token or any token of its input, and — through
  [`expand_at`](#expanderexpand_at) — one level deeper than the code the driver
  found it in. Exceeding it is [`RecursionLimit`](#expanderror).
- **Expansions** (`max_expansions`, default `2^20`) and **tokens**
  (`max_tokens`, default `2^22`), counted in the expander's
  [`Usage`](#usage) until [`reset_usage`](#expanderreset_usage) zeroes it —
  never, for a batch compiler; once per unit of work, for a long-lived host.
  Exceeding either is [`ExpandError::Budget`](#expanderror). A macro that
  doubles its argument every round, or that expands into several calls of
  itself, is stopped here long before it reaches depth 128.

Hygiene cannot see the depth of an invocation rebuilt entirely from captured
source tokens: a macro that writes `$name ! ( $args )` from its own input
reproduces a call identical to the one it came from. A driver that knows where
it found each invocation passes that depth to `expand_at`, which bounds such a
macro by the depth limit; with plain [`expand`](#expanderexpand), the
expansion and token budgets bound it instead. See
[bounding untrusted macros](#guide-bounding-untrusted-macros).

---

## `Tree`

```rust,ignore
pub enum Tree<K> {
    Token { token: Token<K>, ctx: Context },
    Group { open: Token<K>, close: Token<K>, trees: Vec<Tree<K>> },
}
```

A token tree: a single token with its hygiene context, or a delimited group.

**Variants**

- `Token { token, ctx }` — one token (`token.kind`, `token.span`) and the
  [`Context`](#context) it belongs to.
- `Group { open, close, trees }` — an opening delimiter token, the trees between
  the delimiters in source order, and the closing delimiter token. Delimiters
  carry no context; hygiene applies to the tokens inside.

**Trait implementations:** `Clone`, `Debug`, `PartialEq`, `Eq`, `Hash` (each
where `K` implements it).

Building the trees for `f(a, b)`:

```rust
use macro_lang::Tree;
use token_lang::{Span, Token};

let call: Vec<Tree<char>> = vec![
    Tree::token('f', Span::new(0, 1)),
    Tree::Group {
        open: Token::new('(', Span::new(1, 2)),
        close: Token::new(')', Span::new(6, 7)),
        trees: vec![
            Tree::token('a', Span::new(2, 3)),
            Tree::token(',', Span::new(3, 4)),
            Tree::token('b', Span::new(5, 6)),
        ],
    },
];
assert_eq!(call.len(), 2);
```

Grouping a flat token stream from a lexer — the step every front end performs
before expansion:

```rust
use macro_lang::{Context, Tree};
use token_lang::{Span, Token};

fn group(tokens: Vec<Token<char>>) -> Option<Vec<Tree<char>>> {
    let mut stack: Vec<(Token<char>, Vec<Tree<char>>)> = Vec::new();
    let mut top = Vec::new();
    for token in tokens {
        match token.kind {
            '(' => stack.push((token, std::mem::take(&mut top))),
            ')' => {
                let (open, parent) = stack.pop()?; // unbalanced input
                let trees = std::mem::replace(&mut top, parent);
                top.push(Tree::Group { open, close: token, trees });
            }
            _ => top.push(Tree::Token { token, ctx: Context::ROOT }),
        }
    }
    stack.is_empty().then_some(top)
}

let tokens = "a(bc)d"
    .char_indices()
    .map(|(i, c)| Token::new(c, Span::new(i as u32, i as u32 + 1)))
    .collect();
let trees = group(tokens).ok_or("unbalanced")?;
assert_eq!(trees.len(), 3);
assert!(matches!(&trees[1], Tree::Group { trees, .. } if trees.len() == 2));
# Ok::<(), Box<dyn std::error::Error>>(())
```

### `Tree::token`

```rust,ignore
pub const fn token(kind: K, span: Span) -> Tree<K>
```

Builds a single-token tree in the root context — a token written directly in
source.

**Parameters**

- `kind` — the token kind.
- `span` — the token's source span.

```rust
use macro_lang::{Context, Tree};
use token_lang::{Span, Token};

let tree = Tree::token('x', Span::new(4, 5));
assert_eq!(tree, Tree::Token { token: Token::new('x', Span::new(4, 5)), ctx: Context::ROOT });
```

### `Tree::span`

```rust,ignore
pub const fn span(&self) -> Span
```

Returns the source span the tree covers: the token's span, or for a group, from
the start of the opening delimiter to the end of the closing one. Useful for
pointing a diagnostic at a captured argument.

```rust
use macro_lang::Tree;
use token_lang::{Span, Token};

let group: Tree<char> = Tree::Group {
    open: Token::new('[', Span::new(10, 11)),
    close: Token::new(']', Span::new(14, 15)),
    trees: vec![Tree::token('x', Span::new(12, 13))],
};
assert_eq!(group.span(), Span::new(10, 15));
assert_eq!(Tree::token('x', Span::new(2, 3)).span(), Span::new(2, 3));
```

---

## `Pattern`

```rust,ignore
#[non_exhaustive]
pub enum Pattern<K> {
    Token(K),
    Group { open: K, close: K, body: Vec<Pattern<K>> },
    Bind { name: Symbol, fragment: Fragment<K> },
    Repeat { body: Vec<Pattern<K>>, separator: Option<K>, kleene: Kleene },
}
```

One element of a rule's pattern. A pattern is a `Vec<Pattern<K>>` matched against
the invocation left to right; it must consume the whole invocation.

**Variants**

- `Token(kind)` — matches one token whose kind equals `kind`.
- `Group { open, close, body }` — matches one group whose delimiter kinds equal
  `open` and `close` and whose contents match `body` in full.
- `Bind { name, fragment }` — the metavariable `$name:fragment`: captures one
  tree that `fragment` allows. `name` must be unique within the rule.
- `Repeat { body, separator, kleene }` — the repetition `$( body ) sep kleene`:
  matches `body` repeatedly, with a `separator` token between repetitions if one
  is given. The body must consume input on every path
  ([`MacroError::EmptyRepetition`](#macroerror)), and a
  [`Kleene::ZeroOrOne`](#kleene) repetition cannot have a separator
  ([`MacroError::OptionalSeparator`](#macroerror)).

`#[non_exhaustive]`: new kinds of pattern element may be added in a minor
release. Constructing a variant is unaffected; a `match` on `Pattern` needs a
wildcard arm.

**Trait implementations:** `Clone`, `Debug`.

A literal keyword followed by a parenthesized argument — `when ( $cond:tt )`:

```rust
use intern_lang::Interner;
use macro_lang::{Context, Expander, Fragment, Macro, Pattern, Rule, Template, Tree};
use token_lang::{Span, Token};

let mut names = Interner::new();
let cond = names.intern("cond");
let s = Span::new(0, 1);
let when = Macro::new(names.intern("when"), vec![Rule {
    pattern: vec![
        Pattern::Token('w'),
        Pattern::Group {
            open: '(',
            close: ')',
            body: vec![Pattern::Bind { name: cond, fragment: Fragment::Tree }],
        },
    ],
    template: vec![Template::Var(cond)],
}])?;

let input = [
    Tree::token('w', s),
    Tree::Group { open: Token::new('(', s), close: Token::new(')', s), trees: vec![Tree::token('c', s)] },
];
let out = Expander::new().expand(&when, &input, s, Context::ROOT)?;
assert_eq!(out, vec![Tree::token('c', s)]);
# Ok::<(), Box<dyn std::error::Error>>(())
```

A comma-separated list of at least one element — `$($item:tt),+`:

```rust
use intern_lang::Interner;
use macro_lang::{Context, ExpandError, Expander, Fragment, Kleene, Macro, Pattern, Rule, Template, Tree};
use token_lang::Span;
# fn trees(src: &str) -> Vec<Tree<char>> {
#     src.char_indices().map(|(i, c)| Tree::token(c, Span::new(i as u32, i as u32 + 1))).collect()
# }

let mut names = Interner::new();
let item = names.intern("item");
let items = Macro::new(names.intern("items"), vec![Rule {
    pattern: vec![Pattern::Repeat {
        body: vec![Pattern::Bind { name: item, fragment: Fragment::Tree }],
        separator: Some(','),
        kleene: Kleene::OneOrMore,
    }],
    template: vec![Template::Repeat { body: vec![Template::Var(item)], separator: None }],
}])?;

let mut expander = Expander::new();
assert_eq!(expander.expand(&items, &trees("a,b,c"), Span::new(0, 5), Context::ROOT)?.len(), 3);
// `+` needs at least one element.
assert!(matches!(
    expander.expand(&items, &[], Span::new(0, 0), Context::ROOT),
    Err(ExpandError::NoMatch { .. })
));
# Ok::<(), Box<dyn std::error::Error>>(())
```

---

## `Fragment`

```rust,ignore
#[non_exhaustive]
pub enum Fragment<K> {
    Tree,
    Kind(fn(&K) -> bool),
}
```

What a [`Pattern::Bind`](#pattern) may capture — the fragment specifier.
macro-lang knows no grammar, so it cannot offer `expr` or `ty`; it offers the
structural specifier every macro system has, plus a token-level test the
language supplies.

**Variants**

- `Tree` — any single token tree: one token, or one whole group. The `tt`
  specifier.
- `Kind(test)` — a single token (never a group) for which `test(&kind)` returns
  `true`. Covers specifiers such as `ident` and `literal`:
  `Fragment::Kind(Kind::is_ident)`.

`#[non_exhaustive]`: new fragment kinds may be added in a minor release; a
`match` on `Fragment` needs a wildcard arm.

**Trait implementations:** `Clone`, `Copy`, `Debug`.

An `ident`-style fragment that only accepts letters:

```rust
use intern_lang::Interner;
use macro_lang::{Context, ExpandError, Expander, Fragment, Macro, Pattern, Rule, Template, Tree};
use token_lang::Span;

let mut names = Interner::new();
let name = names.intern("name");
let ident = Macro::new(names.intern("ident"), vec![Rule {
    pattern: vec![Pattern::Bind { name, fragment: Fragment::Kind(|c: &char| c.is_alphabetic()) }],
    template: vec![Template::Var(name)],
}])?;

let mut expander = Expander::new();
let ok = expander.expand(&ident, &[Tree::token('q', Span::new(0, 1))], Span::new(0, 1), Context::ROOT);
assert!(ok.is_ok());

let err = expander.expand(&ident, &[Tree::token('7', Span::new(0, 1))], Span::new(0, 1), Context::ROOT);
assert_eq!(err, Err(ExpandError::NoMatch { span: Span::new(0, 1) }));
# Ok::<(), Box<dyn std::error::Error>>(())
```

`Fragment::Tree` captures a whole group, delimiters included:

```rust
use intern_lang::Interner;
use macro_lang::{Context, Expander, Fragment, Macro, Pattern, Rule, Template, Tree};
use token_lang::{Span, Token};

let mut names = Interner::new();
let t = names.intern("t");
let id = Macro::new(names.intern("id"), vec![Rule {
    pattern: vec![Pattern::Bind { name: t, fragment: Fragment::Tree }],
    template: vec![Template::Var(t)],
}])?;

let s = Span::new(0, 1);
let group = Tree::Group {
    open: Token::new('(', s),
    close: Token::new(')', s),
    trees: vec![Tree::token('a', s), Tree::token('b', s)],
};
let out = Expander::new().expand(&id, &[group.clone()], s, Context::ROOT)?;
assert_eq!(out, vec![group]);
# Ok::<(), Box<dyn std::error::Error>>(())
```

---

## `Kleene`

```rust,ignore
pub enum Kleene {
    ZeroOrMore, // *
    OneOrMore,  // +
    ZeroOrOne,  // ?
}
```

How many times a [`Pattern::Repeat`](#pattern) may match.

**Trait implementations:** `Clone`, `Copy`, `Debug`, `PartialEq`, `Eq`, `Hash`.

### `Kleene::allows`

```rust,ignore
pub const fn allows(self, count: usize) -> bool
```

Returns `true` if a repetition may match exactly `count` times. Handy for a
front end validating its own syntax, or for tests.

**Parameters**

- `count` — a number of repetitions.

```rust
use macro_lang::Kleene;

assert!(Kleene::ZeroOrMore.allows(0));
assert!(Kleene::OneOrMore.allows(5));
assert!(!Kleene::OneOrMore.allows(0));
assert!(Kleene::ZeroOrOne.allows(1));
assert!(!Kleene::ZeroOrOne.allows(2));
```

---

## `Template`

```rust,ignore
#[non_exhaustive]
pub enum Template<K> {
    Token { token: Token<K>, ctx: Context },
    Group { open: Token<K>, close: Token<K>, body: Vec<Template<K>> },
    Var(Symbol),
    Repeat { body: Vec<Template<K>>, separator: Option<Token<K>> },
    RepeatSeparated { body: Vec<Template<K>>, separator: Token<K>, ctx: Context }, // 1.1.0
}
```

One element of a rule's template. A template is a `Vec<Template<K>>` written out
in order when the rule matches.

**Variants**

- `Token { token, ctx }` — a literal token. It is written with a context minted
  for the expansion; `ctx` is the context the token has in the macro definition
  — [`Context::ROOT`](#context) for a macro written in source, or the context an
  earlier expansion gave it for a macro defined by a macro.
- `Group { open, close, body }` — a literal group, written with its delimiters
  and its expanded body.
- `Var(name)` — the tree captured by metavariable `name`, substituted with its
  contexts unchanged. Must be bound by the rule's pattern
  ([`MacroError::UnboundVariable`](#macroerror)).
- `Repeat { body, separator }` — `body` written once per captured repetition,
  with `separator` written between repetitions. The separator is marked as a
  token defined in the root context — exact for a macro written in source. The
  iteration count comes from the metavariables used inside (see
  [lockstep](#repetition-depth-and-lockstep)).
- `RepeatSeparated { body, separator, ctx }` — *Added in 1.1.0.* The same, with
  the separator's definition context given explicitly, like
  `Token { ctx, .. }`. The separator is marked exactly like a literal token
  defined in `ctx`: it shares the context the expansion mints for every other
  literal defined there, and [`Origin::parent`](#origin) reports `ctx`. Use it
  when lowering a macro definition that an earlier expansion produced, so its
  separators bind and resolve like the tokens around them. With `ctx` equal to
  `Context::ROOT` it behaves exactly like `Repeat` with `Some(separator)`.

`#[non_exhaustive]`: new kinds of template element may be added in a minor
release. Constructing a variant is unaffected; a `match` on `Template` needs a
wildcard arm.

**Trait implementations:** `Clone`, `Debug`, `PartialEq`, `Eq`, `Hash`.

A template mixing literals, a group, and a separated repetition —
`( $($arg)+* )`:

```rust
use intern_lang::Interner;
use macro_lang::{Context, Expander, Fragment, Kleene, Macro, Pattern, Rule, Template, Tree};
use token_lang::{Span, Token};
# fn trees(src: &str) -> Vec<Tree<char>> {
#     src.char_indices().map(|(i, c)| Tree::token(c, Span::new(i as u32, i as u32 + 1))).collect()
# }
# fn text(trees: &[Tree<char>]) -> String {
#     trees.iter().map(|t| match t {
#         Tree::Token { token, .. } => token.kind.to_string(),
#         Tree::Group { open, close, trees } => format!("{}{}{}", open.kind, text(trees), close.kind),
#     }).collect()
# }

let mut names = Interner::new();
let arg = names.intern("arg");
let s = Span::new(0, 1);
let sum = Macro::new(names.intern("sum"), vec![Rule {
    pattern: vec![Pattern::Repeat {
        body: vec![Pattern::Bind { name: arg, fragment: Fragment::Tree }],
        separator: None,
        kleene: Kleene::ZeroOrMore,
    }],
    template: vec![Template::Group {
        open: Token::new('(', s),
        close: Token::new(')', s),
        body: vec![Template::Repeat {
            body: vec![Template::Var(arg)],
            separator: Some(Token::new('+', s)),
        }],
    }],
}])?;

let out = Expander::new().expand(&sum, &trees("abc"), s, Context::ROOT)?;
assert_eq!(text(&out), "(a+b+c)");
# Ok::<(), Box<dyn std::error::Error>>(())
```

A metavariable of depth 0 used inside a repetition is repeated with it —
`$head:tt $($x:tt)*  =>  $( $head $x )*`:

```rust
use intern_lang::Interner;
use macro_lang::{Context, Expander, Fragment, Kleene, Macro, Pattern, Rule, Template, Tree};
use token_lang::Span;
# fn trees(src: &str) -> Vec<Tree<char>> {
#     src.char_indices().map(|(i, c)| Tree::token(c, Span::new(i as u32, i as u32 + 1))).collect()
# }
# fn text(trees: &[Tree<char>]) -> String {
#     trees.iter().map(|t| match t {
#         Tree::Token { token, .. } => token.kind.to_string(),
#         Tree::Group { open, close, trees } => format!("{}{}{}", open.kind, text(trees), close.kind),
#     }).collect()
# }

let mut names = Interner::new();
let (head, x) = (names.intern("head"), names.intern("x"));
let distribute = Macro::new(names.intern("distribute"), vec![Rule {
    pattern: vec![
        Pattern::Bind { name: head, fragment: Fragment::Tree },
        Pattern::Repeat {
            body: vec![Pattern::Bind { name: x, fragment: Fragment::Tree }],
            separator: None,
            kleene: Kleene::ZeroOrMore,
        },
    ],
    template: vec![Template::Repeat {
        body: vec![Template::Var(head), Template::Var(x)],
        separator: None,
    }],
}])?;

let out = Expander::new().expand(&distribute, &trees("fabc"), Span::new(0, 4), Context::ROOT)?;
assert_eq!(text(&out), "fafbfc");
# Ok::<(), Box<dyn std::error::Error>>(())
```

A separator carrying its definition context — here a context an earlier
expansion minted, as it would be for a macro defined by a macro:

```rust
use intern_lang::Interner;
use macro_lang::{Context, Expander, Fragment, Kleene, Macro, Pattern, Rule, Template, Tree};
use token_lang::{Span, Token};

let mut names = Interner::new();
let s = Span::new(0, 1);
let mut expander = Expander::new();

// A context from an earlier expansion, standing in for the definition's.
let maker = Macro::new(names.intern("maker"), vec![Rule {
    pattern: vec![],
    template: vec![Template::token('k', s)],
}])?;
let Some(Tree::Token { ctx: def, .. }) = expander.expand(&maker, &[], s, Context::ROOT)?.first().cloned() else {
    return Err("expected a token".into());
};

// join!($($x:tt)*) => t $($x);*   with `t` and `;` both defined in `def`
let x = names.intern("x");
let join = Macro::new(names.intern("join"), vec![Rule {
    pattern: vec![Pattern::Repeat {
        body: vec![Pattern::Bind { name: x, fragment: Fragment::Tree }],
        separator: None,
        kleene: Kleene::ZeroOrMore,
    }],
    template: vec![
        Template::Token { token: Token::new('t', s), ctx: def },
        Template::RepeatSeparated {
            body: vec![Template::Var(x)],
            separator: Token::new(';', s),
            ctx: def,
        },
    ],
}])?;

let input = [Tree::token('a', s), Tree::token('b', s)];
let out = expander.expand(&join, &input, s, Context::ROOT)?;
let ctx_of = |i: usize| match out.get(i) {
    Some(Tree::Token { ctx, .. }) => Some(*ctx),
    _ => None,
};
// `t` (index 0) and `;` (index 2) share one minted context, parented by `def`.
assert_eq!(ctx_of(0), ctx_of(2));
let minted = ctx_of(2).ok_or("expected the separator")?;
assert_eq!(expander.origin(minted).ok_or("not minted")?.parent, def);
# Ok::<(), Box<dyn std::error::Error>>(())
```

### `Template::token`

```rust,ignore
pub const fn token(kind: K, span: Span) -> Template<K>
```

Builds a literal template token defined in the root context — the common case
for a macro written directly in source.

**Parameters**

- `kind` — the token kind to write.
- `span` — the token's span in the macro definition. Output tokens keep it, so a
  diagnostic on macro-introduced code can point into the definition.

```rust
use macro_lang::{Context, Template};
use token_lang::{Span, Token};

assert_eq!(
    Template::token(';', Span::new(8, 9)),
    Template::Token { token: Token::new(';', Span::new(8, 9)), ctx: Context::ROOT },
);
```

---

## `Rule`

```rust,ignore
pub struct Rule<K> {
    pub pattern: Vec<Pattern<K>>,
    pub template: Vec<Template<K>>,
}
```

One arm of a macro: a pattern and the template it expands to. Plain data;
[`Macro::new`](#macronew) validates and compiles it.

**Fields**

- `pattern` — what an invocation must look like; it must match all of the
  invocation's trees.
- `template` — what the invocation expands to.

**Trait implementations:** `Clone`, `Debug`.

`($x:tt) => { $x $x }`:

```rust
use intern_lang::Interner;
use macro_lang::{Fragment, Pattern, Rule, Template};

let mut names = Interner::new();
let x = names.intern("x");
let rule: Rule<char> = Rule {
    pattern: vec![Pattern::Bind { name: x, fragment: Fragment::Tree }],
    template: vec![Template::Var(x), Template::Var(x)],
};
assert_eq!(rule.pattern.len(), 1);
assert_eq!(rule.template.len(), 2);
```

An empty rule — `() => {}` — matches only an empty invocation and expands to
nothing:

```rust
use intern_lang::Interner;
use macro_lang::{Context, Expander, Macro, Rule};
use token_lang::Span;

let mut names = Interner::new();
let nothing = Macro::new(names.intern("nothing"), vec![Rule { pattern: vec![], template: vec![] }])?;
let out = Expander::<char>::new().expand(&nothing, &[], Span::new(0, 10), Context::ROOT)?;
assert!(out.is_empty());
# Ok::<(), Box<dyn std::error::Error>>(())
```

---

## `Macro`

```rust,ignore
pub struct Macro<K> { /* private */ }
```

A validated, compiled macro: a name and an ordered list of rules. Building one
does all the work that does not depend on an invocation — every static check
(see [`MacroError`](#macroerror)) and the compilation of each pattern into a
matching automaton and each template into a flat program. A macro is immutable
once built and can be expanded any number of times by any number of expanders.

**Trait implementations:** `Clone` (where `K: Clone`), `Debug` (prints the name
and rule count), `Send` and `Sync` (where `K` is).

### `Macro::new`

```rust,ignore
pub fn new(name: Symbol, rules: Vec<Rule<K>>) -> Result<Macro<K>, MacroError>
```

Validates and compiles a macro.

**Parameters**

- `name` — the macro's name, recorded in the [`Origin`](#origin) of every
  context its expansions mint.
- `rules` — the arms, in the order they are tried.

**Errors** — the first problem found, checking rules in order:

| Error | Cause |
|---|---|
| [`NoRules`](#macroerror) | `rules` is empty. |
| [`DuplicateBinding`](#macroerror) | A pattern binds a name twice. |
| [`EmptyRepetition`](#macroerror) | A pattern repetition can match nothing. |
| [`OptionalSeparator`](#macroerror) | A `?` repetition has a separator. |
| [`UnboundVariable`](#macroerror) | A template uses a name its pattern does not bind. |
| [`StillRepeating`](#macroerror) | A template uses a repeating metavariable outside enough repetitions. |
| [`NoRepeatingVariable`](#macroerror) | A template repetition has nothing to repeat over. |

A two-rule macro — `()` expands to `0`, anything else passes through:

```rust
use intern_lang::Interner;
use macro_lang::{Context, Expander, Fragment, Kleene, Macro, Pattern, Rule, Template, Tree};
use token_lang::Span;

let mut names = Interner::new();
let t = names.intern("t");
let or_zero = Macro::new(names.intern("or_zero"), vec![
    Rule { pattern: vec![], template: vec![Template::token('0', Span::new(0, 1))] },
    Rule {
        pattern: vec![Pattern::Repeat {
            body: vec![Pattern::Bind { name: t, fragment: Fragment::Tree }],
            separator: None,
            kleene: Kleene::OneOrMore,
        }],
        template: vec![Template::Repeat { body: vec![Template::Var(t)], separator: None }],
    },
])?;

let mut expander = Expander::new();
let zero = expander.expand(&or_zero, &[], Span::new(0, 0), Context::ROOT)?;
assert!(matches!(&zero[..], [Tree::Token { token, .. }] if token.kind == '0'));
let kept = expander.expand(&or_zero, &[Tree::token('k', Span::new(0, 1))], Span::new(0, 1), Context::ROOT)?;
assert_eq!(kept, vec![Tree::token('k', Span::new(0, 1))]);
# Ok::<(), Box<dyn std::error::Error>>(())
```

A definition error, caught before any invocation:

```rust
use intern_lang::Interner;
use macro_lang::{Fragment, Kleene, Macro, MacroError, Pattern, Rule, Template};

let mut names = Interner::new();
let x = names.intern("x");

// `$x` repeats in the pattern but is used bare in the template.
let err = Macro::<char>::new(names.intern("m"), vec![Rule {
    pattern: vec![Pattern::Repeat {
        body: vec![Pattern::Bind { name: x, fragment: Fragment::Tree }],
        separator: None,
        kleene: Kleene::ZeroOrMore,
    }],
    template: vec![Template::Var(x)],
}]).unwrap_err();
assert_eq!(err, MacroError::StillRepeating { rule: 0, name: x });
```

### `Macro::name`

```rust,ignore
pub const fn name(&self) -> Symbol
```

Returns the macro's name — the key an expansion driver usually stores the macro
under.

```rust
use intern_lang::Interner;
use macro_lang::{Macro, Rule};

let mut names = Interner::new();
let mac: Macro<char> = Macro::new(names.intern("empty"), vec![Rule { pattern: vec![], template: vec![] }])?;
assert_eq!(names.resolve(mac.name()), Some("empty"));
# Ok::<(), macro_lang::MacroError>(())
```

---

## `Expander`

```rust,ignore
pub struct Expander<K> { /* private */ }
```

Runs expansions and keeps the hygiene record of every one. An expander owns the
**hygiene table** — every [`Context`](#context) it has minted and the expansion
that minted it — a **[`Budget`](#budget)** bounding the work it may do (see
[termination and budgets](#termination-and-budgets)), and **pooled scratch
buffers** reused by every call, so steady-state expansion allocates only the
output trees.

Use one expander for a whole compilation (or one per thread), so every context in
the program comes from one table. The expander does not find invocations or look
macros up by name; that is the expansion driver's job (see
[writing a front end](#guide-writing-a-front-end)).

**Trait implementations:** `Default` (same as [`new`](#expandernew)), `Debug`
(a summary: the limit and the number of contexts and expansions recorded; the
exact format is not part of the stability contract), `Send` and `Sync` (where
`K` is).

### `Expander::DEFAULT_LIMIT`

```rust,ignore
pub const DEFAULT_LIMIT: u32 = 128;
```

The recursion limit [`Expander::new`](#expandernew) uses: 128 nested expansions,
the same default as rustc's `recursion_limit`. Equal to
[`Budget::DEFAULT`](#budgetdefault)`.max_depth`.

```rust
use macro_lang::Expander;

assert_eq!(Expander::<char>::DEFAULT_LIMIT, 128);
```

### `Expander::new`

```rust,ignore
pub fn new() -> Expander<K>
```

Creates an expander with the [default budget](#budgetdefault) — depth 128,
`2^20` expansions, `2^22` tokens — and an empty hygiene table.

```rust
use macro_lang::Expander;

let expander: Expander<char> = Expander::new();
assert_eq!(expander.limit(), Expander::<char>::DEFAULT_LIMIT);
```

### `Expander::with_limit`

```rust,ignore
pub fn with_limit(limit: u32) -> Expander<K>
```

Creates an expander that allows invocations nested at most `limit` expansions
deep, with the rest of the [default budget](#budgetdefault). Equivalent to
`Expander::with_budget(Budget::DEFAULT.with_max_depth(limit))`.

**Parameters**

- `limit` — the maximum nesting depth. An invocation runs one level deeper than
  the deepest expansion that produced its name token (`call_context`) or any
  token of its input, and, through [`expand_at`](#expanderexpand_at), one level
  deeper than the code the driver found it in. A limit of `0` rejects every
  expansion.

A macro with no base case is stopped instead of looping:

```rust
use intern_lang::Interner;
use macro_lang::{Context, ExpandError, Expander, Macro, Rule, Template, Tree};
use token_lang::Span;

let mut names = Interner::new();
// again!() => again — the output is one token, which the loop below treats
// as a fresh invocation of the same macro.
let again = Macro::new(names.intern("again"), vec![Rule {
    pattern: vec![],
    template: vec![Template::token('g', Span::new(0, 1))],
}])?;

let mut expander = Expander::with_limit(4);
let mut call_context = Context::ROOT;
let err = loop {
    match expander.expand(&again, &[], Span::new(0, 1), call_context) {
        Ok(out) => match out.first() {
            Some(Tree::Token { ctx, .. }) => call_context = *ctx,
            _ => break None,
        },
        Err(err) => break Some(err),
    }
};
assert_eq!(err, Some(ExpandError::RecursionLimit { limit: 4 }));
# Ok::<(), Box<dyn std::error::Error>>(())
```

### `Expander::limit`

```rust,ignore
pub const fn limit(&self) -> u32
```

Returns the recursion limit — the budget's `max_depth`.

```rust
use macro_lang::Expander;

assert_eq!(Expander::<char>::with_limit(16).limit(), 16);
```

### `Expander::with_budget`

```rust,ignore
pub fn with_budget(budget: Budget) -> Expander<K>
```

*Added in 1.1.0.* Creates an expander that enforces `budget`.

```rust
use macro_lang::{Budget, Expander};

let budget = Budget::DEFAULT.with_max_expansions(10_000).with_max_tokens(100_000);
let expander: Expander<char> = Expander::with_budget(budget);
assert_eq!(expander.budget(), budget);
assert_eq!(expander.limit(), 128);
```

### `Expander::budget`

```rust,ignore
pub const fn budget(&self) -> Budget
```

*Added in 1.1.0.* Returns the budget the expander enforces.

```rust
use macro_lang::{Budget, Expander};

assert_eq!(Expander::<char>::new().budget(), Budget::DEFAULT);
assert_eq!(Expander::<char>::with_limit(9).budget(), Budget::DEFAULT.with_max_depth(9));
```

### `Expander::set_budget`

```rust,ignore
pub fn set_budget(&mut self, budget: Budget)
```

*Added in 1.1.0.* Replaces the budget from now on. Work already done stays
counted — the expansion and token limits apply to the [`usage`](#expanderusage)
since the last [reset](#expanderreset_usage) — so raising them lets the current
unit of work continue, and lowering them below what it has already done makes
every further expansion fail with `ExpandError::Budget`.

```rust
use intern_lang::Interner;
use macro_lang::{Budget, Context, ExpandError, Expander, Limit, Macro, Rule};
use token_lang::Span;

let mut names = Interner::new();
let empty = Macro::<char>::new(names.intern("empty"), vec![Rule { pattern: vec![], template: vec![] }])?;
let s = Span::new(0, 1);

let mut expander = Expander::with_budget(Budget::DEFAULT.with_max_expansions(1));
assert!(expander.expand(&empty, &[], s, Context::ROOT).is_ok());
assert_eq!(expander.expand(&empty, &[], s, Context::ROOT),
           Err(ExpandError::Budget(Limit::Expansions { max: 1 })));

expander.set_budget(Budget::DEFAULT.with_max_expansions(2));
assert!(expander.expand(&empty, &[], s, Context::ROOT).is_ok());
# Ok::<(), Box<dyn std::error::Error>>(())
```

### `Expander::usage`

```rust,ignore
pub const fn usage(&self) -> Usage
```

*Added in 1.1.0.* Returns the work charged against the budget since the
expander was created or [`reset_usage`](#expanderreset_usage) was last called:
successful expansions and the tokens they wrote. Failed expansions are never
charged.

```rust
use intern_lang::Interner;
use macro_lang::{Context, Expander, Macro, Rule, Template, Usage};
use token_lang::Span;

let mut names = Interner::new();
let s = Span::new(0, 1);
let three = Macro::new(names.intern("three"), vec![Rule {
    pattern: vec![],
    template: "abc".chars().map(|c| Template::token(c, s)).collect(),
}])?;

let mut expander = Expander::new();
assert_eq!(expander.usage(), Usage::NONE);
expander.expand(&three, &[], s, Context::ROOT)?;
assert_eq!((expander.usage().expansions, expander.usage().tokens), (1, 3));
# Ok::<(), Box<dyn std::error::Error>>(())
```

### `Expander::reset_usage`

```rust,ignore
pub fn reset_usage(&mut self)
```

*Added in 1.1.0.* Zeroes the [`usage`](#expanderusage) counters, so the next
unit of work gets the full budget again. Nothing else changes: the budget,
every minted context, and every [`Origin`](#origin) stay as they were, so trees
produced before the reset remain valid, and nesting depth (which is hygiene,
not usage) is not reset either. This is how a long-lived host — a language
server, a REPL, a watch-mode compiler — keeps the budget per unit of work
rather than per process: it calls `reset_usage()` once per document parse, per
REPL entry, or per compilation (see
[bounding untrusted macros](#guide-bounding-untrusted-macros)). The hygiene
table still only grows; after billions of expansions a host sees
`ExpandError::ContextOverflow` and starts a fresh expander.

```rust
use intern_lang::Interner;
use macro_lang::{Budget, Context, ExpandError, Expander, Limit, Macro, Rule, Template, Usage};
use token_lang::Span;

let mut names = Interner::new();
let s = Span::new(0, 1);
let one = Macro::new(names.intern("one"), vec![Rule {
    pattern: vec![],
    template: vec![Template::token('k', s)],
}])?;

// A REPL that allows two expansions per entry, on one expander for the session.
let mut expander = Expander::with_budget(Budget::DEFAULT.with_max_expansions(2));
for _entry in 0..3 {
    expander.reset_usage();
    expander.expand(&one, &[], s, Context::ROOT)?;
    expander.expand(&one, &[], s, Context::ROOT)?;
    assert_eq!(expander.expand(&one, &[], s, Context::ROOT),
               Err(ExpandError::Budget(Limit::Expansions { max: 2 })));
}
expander.reset_usage();
assert_eq!(expander.usage(), Usage::NONE);
# Ok::<(), Box<dyn std::error::Error>>(())
```

### `Expander::expand`

```rust,ignore
pub fn expand(
    &mut self,
    mac: &Macro<K>,
    input: &[Tree<K>],
    call_site: Span,
    call_context: Context,
) -> Result<Vec<Tree<K>>, ExpandError>
where
    K: Clone + PartialEq,
```

Expands one invocation of `mac`. The rules are tried in order against `input`;
the first rule whose pattern matches all of it is transcribed. Every literal
token the template writes receives a context minted for this expansion; every
captured tree is substituted with its contexts untouched.

**Parameters**

- `mac` — the macro being invoked.
- `input` — the invocation's argument trees: the contents between its
  delimiters, without the delimiters.
- `call_site` — the span of the whole invocation. Recorded in the
  [`Origin`](#origin) of every minted context, and reported by
  [`ExpandError::NoMatch`](#expanderror) for an empty invocation.
- `call_context` — the context of the invocation itself, normally that of the
  macro-name token. [`Context::ROOT`](#context) for an invocation written in
  source; the name token's context for one produced by an earlier expansion, so
  nesting depth is tracked.

`expand` is [`expand_at`](#expanderexpand_at) with a depth of `0`: the
invocation's depth is inferred from hygiene alone — from `call_context` and the
contexts of the input tokens. An invocation rebuilt entirely from captured
source tokens looks exactly like the original call, so for that shape the
[expansion and token budgets](#termination-and-budgets), not the depth limit,
are what stop a runaway macro. A driver that knows where it found each
invocation should use `expand_at`.

**Returns** the expanded trees.

**Errors**

| Error | Cause |
|---|---|
| [`RecursionLimit`](#expanderror) | The invocation is nested deeper than the [limit](#expanderlimit). |
| [`Budget`](#expanderror) | The expander has already performed `max_expansions` expansions, or this one would take the tokens written past `max_tokens`. The token check is made before each token is written, so an oversized expansion is refused without building its output. |
| [`NoMatch`](#expanderror) | No rule matches. |
| [`Ambiguous`](#expanderror) | A rule matches in more than one way. |
| [`RepetitionMismatch`](#expanderror) | Lockstep metavariables captured different numbers of trees. |
| [`ContextOverflow`](#expanderror) | The context space is exhausted. |

On error the expander is left exactly as it was: no contexts are minted and no
expansion is recorded.

Rule selection — rules are tried in order:

```rust
use intern_lang::Interner;
use macro_lang::{Context, Expander, Fragment, Macro, Pattern, Rule, Template, Tree};
use token_lang::Span;
# fn trees(src: &str) -> Vec<Tree<char>> {
#     src.char_indices().map(|(i, c)| Tree::token(c, Span::new(i as u32, i as u32 + 1))).collect()
# }
# fn text(trees: &[Tree<char>]) -> String {
#     trees.iter().map(|t| match t {
#         Tree::Token { token, .. } => token.kind.to_string(),
#         Tree::Group { open, close, trees } => format!("{}{}{}", open.kind, text(trees), close.kind),
#     }).collect()
# }

let mut names = Interner::new();
let x = names.intern("x");
let s = Span::new(0, 1);
let pick = Macro::new(names.intern("pick"), vec![
    Rule { pattern: vec![Pattern::Token('a')], template: vec![Template::token('A', s)] },
    Rule {
        pattern: vec![Pattern::Bind { name: x, fragment: Fragment::Tree }],
        template: vec![Template::Var(x), Template::Var(x)],
    },
])?;

let mut expander = Expander::new();
assert_eq!(text(&expander.expand(&pick, &trees("a"), s, Context::ROOT)?), "A");
assert_eq!(text(&expander.expand(&pick, &trees("b"), s, Context::ROOT)?), "bb");
# Ok::<(), Box<dyn std::error::Error>>(())
```

Re-expanding output that contains another invocation — pass the context of the
inner invocation's name token as `call_context`:

```rust
use intern_lang::Interner;
use macro_lang::{Context, Expander, Macro, Rule, Template, Tree};
use token_lang::Span;

let mut names = Interner::new();
let s = Span::new(0, 1);
// outer!() => inner      inner!() => done
let outer = Macro::new(names.intern("outer"), vec![Rule { pattern: vec![], template: vec![Template::token('i', s)] }])?;
let inner = Macro::new(names.intern("inner"), vec![Rule { pattern: vec![], template: vec![Template::token('d', s)] }])?;

let mut expander = Expander::new();
let first = expander.expand(&outer, &[], s, Context::ROOT)?;
let Some(Tree::Token { ctx: inner_call, .. }) = first.first() else {
    return Err("expected one token".into());
};
let second = expander.expand(&inner, &[], s, *inner_call)?;
let Some(Tree::Token { ctx, .. }) = second.first() else {
    return Err("expected one token".into());
};
// The second expansion records that it was invoked from code the first produced.
assert_eq!(expander.origin(*ctx).ok_or("unknown context")?.call_context, *inner_call);
# Ok::<(), Box<dyn std::error::Error>>(())
```

### `Expander::expand_at`

```rust,ignore
pub fn expand_at(
    &mut self,
    mac: &Macro<K>,
    input: &[Tree<K>],
    call_site: Span,
    call_context: Context,
    depth: u32,
) -> Result<Vec<Tree<K>>, ExpandError>
where
    K: Clone + PartialEq,
```

*Added in 1.1.0.* [`expand`](#expanderexpand), with the driver also saying
where it found the invocation. The invocation runs one level deeper than the
deepest of `depth`, the expansion that produced `call_context`, and the
expansions that produced any input token; so the recursion limit bounds every
chain of nested expansions the driver performs, whatever the macros write —
including a macro that rebuilds its own invocation from captured tokens.

**Parameters**

As for [`expand`](#expanderexpand), plus:

- `depth` — the nesting depth of the code the invocation was found in: `0` for
  source, and `d + 1` for an invocation found in the output of a call made with
  depth `d`.

**Errors** as for [`expand`](#expanderexpand); on error the expander is left
exactly as it was.

`again!($n:tt $b:tt) => $n $b ($n $b)` rebuilds its own invocation from
captured tokens, so hygiene sees no nesting at all; the tracked depth still
stops it:

```rust
use intern_lang::Interner;
use macro_lang::{Context, ExpandError, Expander, Fragment, Macro, Pattern, Rule, Template, Tree};
use token_lang::{Span, Token};

let mut names = Interner::new();
let (n, b) = (names.intern("n"), names.intern("b"));
let tt = |name| Pattern::Bind { name, fragment: Fragment::Tree };
let s = Span::new(0, 1);
let again = Macro::new(names.intern("again"), vec![Rule {
    pattern: vec![tt(n), tt(b)],
    template: vec![
        Template::Var(n),
        Template::Var(b),
        Template::Group {
            open: Token::new('(', s),
            close: Token::new(')', s),
            body: vec![Template::Var(n), Template::Var(b)],
        },
    ],
}])?;

let mut expander = Expander::with_limit(8);
// `a ! (a !)`, with `a` naming `again`: its arguments are `a !`.
let mut args = vec![Tree::token('a', s), Tree::token('!', s)];
let mut depth = 0;
let err = loop {
    match expander.expand_at(&again, &args, s, Context::ROOT, depth) {
        Ok(out) => match out.get(2) {
            // The output invokes `again` once more: recurse into it.
            Some(Tree::Group { trees, .. }) => { args = trees.clone(); depth += 1; }
            _ => break None,
        },
        Err(err) => break Some(err),
    }
};
assert_eq!(err, Some(ExpandError::RecursionLimit { limit: 8 }));
# Ok::<(), Box<dyn std::error::Error>>(())
```

### `Expander::origin`

```rust,ignore
pub fn origin(&self, ctx: Context) -> Option<Origin>
```

Reports which expansion minted `ctx`, or `None` for [`Context::ROOT`](#context)
and for contexts this expander did not mint.

**Parameters**

- `ctx` — the context to look up.

**Returns** the [`Origin`](#origin): the context the token had in the macro
definition, the macro's name, the call site, and the call's own context.

```rust
use intern_lang::Interner;
use macro_lang::{Context, Expander, Macro, Rule, Template, Tree};
use token_lang::Span;

let mut names = Interner::new();
let make_tmp = names.intern("make_tmp");
let mac = Macro::new(make_tmp, vec![Rule {
    pattern: vec![],
    template: vec![Template::token('t', Span::new(40, 41))],
}])?;

let mut expander = Expander::new();
let out = expander.expand(&mac, &[], Span::new(3, 14), Context::ROOT)?;
let Some(Tree::Token { ctx, .. }) = out.first() else {
    return Err("expected one token".into());
};

let origin = expander.origin(*ctx).ok_or("not minted here")?;
assert_eq!(origin.macro_name, make_tmp);
assert_eq!(origin.call_site, Span::new(3, 14));
assert_eq!(origin.parent, Context::ROOT);
assert_eq!(expander.origin(Context::ROOT), None);
# Ok::<(), Box<dyn std::error::Error>>(())
```

Building an expansion backtrace by following `call_context` outward:

```rust
use intern_lang::Interner;
use macro_lang::{Context, Expander, Macro, Rule, Template, Tree};
use token_lang::Span;

let mut names = Interner::new();
let s = Span::new(0, 1);
let step = Macro::new(names.intern("step"), vec![Rule { pattern: vec![], template: vec![Template::token('s', s)] }])?;

// Expand three levels deep, each call made from the previous output.
let mut expander = Expander::new();
let mut call = Context::ROOT;
for _ in 0..3 {
    let out = expander.expand(&step, &[], s, call)?;
    let Some(Tree::Token { ctx, .. }) = out.first() else {
        return Err("expected one token".into());
    };
    call = *ctx;
}

let mut depth = 0;
let mut ctx = call;
while let Some(origin) = expander.origin(ctx) {
    depth += 1;
    ctx = origin.call_context;
}
assert_eq!(depth, 3);
# Ok::<(), Box<dyn std::error::Error>>(())
```

---

## `Budget`

```rust,ignore
#[non_exhaustive]
pub struct Budget {
    pub max_depth: u32,
    pub max_expansions: usize,
    pub max_tokens: usize,
}
```

*Added in 1.1.0.* The limits an [`Expander`](#expander) enforces on the work it
does. Every expander has one; [`Expander::new`](#expandernew) uses
[`Budget::DEFAULT`](#budgetdefault).

| Field | Limits | Default | Exceeding it reports |
|---|---|---:|---|
| `max_depth` | How deeply invocations may nest (source is depth 1). | 128 | [`ExpandError::RecursionLimit`](#expanderror) |
| `max_expansions` | Successful expansions since creation or the last `reset_usage`. | 1 048 576 (`2^20`) | [`ExpandError::Budget`](#expanderror)`(Limit::Expansions { .. })` |
| `max_tokens` | Tokens written since creation or the last `reset_usage`. | 4 194 304 (`2^22`) | [`ExpandError::Budget`](#expanderror)`(Limit::Tokens { .. })` |

- **Cumulative until reset.** The expansion and token counts accumulate in
  the expander's [`Usage`](#usage). A batch compiler (one expander per
  compilation) never resets, and the budget bounds the compilation's total
  macro output. A long-lived host — language server, REPL, watch-mode
  compiler — calls [`reset_usage`](#expanderreset_usage) once per unit of work,
  so the budget bounds each document, entry, or rebuild rather than the
  process. A unit that legitimately needs more raises the budget with
  [`set_budget`](#expanderset_budget).
- **How tokens are counted.** One per token written, two per group (its
  delimiters), and every token of a substituted capture. Each token is charged
  *before* it is written, so an expansion that would pass `max_tokens` stops at
  the limit instead of first building its output — even a single enormous
  capture is refused without being copied.
- **Failures are free.** A failed expansion is not charged; like every error,
  it leaves the expander exactly as it was.
- **What the defaults cost.** They are well above what typical programs
  expand to, and they stop a runaway macro quickly. Measured with
  `examples/budget` (release build, Windows x86_64, an 8-byte token kind): a
  doubling macro stops in about 0.2 s with a whole-process peak of about
  400 MB, and a self-rebuilding macro under plain `expand` in about 0.15 s.
  Memory grows in proportion to `max_tokens` and the size of the token kind;
  lower `max_tokens` where that is too much.

`#[non_exhaustive]`: further limits may be added in a minor release. Start from
`Budget::DEFAULT` (or `Budget::default()`) and adjust it with the `with_*`
methods, or assign the public fields.

**Trait implementations:** `Clone`, `Copy`, `Debug`, `Default` (=
`Budget::DEFAULT`), `PartialEq`, `Eq`, `Hash`.

```rust
use macro_lang::{Budget, Expander};

let mut budget = Budget::DEFAULT.with_max_depth(32);
budget.max_tokens = 1 << 16;
let expander: Expander<char> = Expander::with_budget(budget);
assert_eq!(expander.limit(), 32);
assert_eq!(expander.budget().max_tokens, 65_536);
```

### `Budget::DEFAULT`

```rust,ignore
pub const DEFAULT: Budget = Budget { max_depth: 128, max_expansions: 1 << 20, max_tokens: 1 << 22 };
```

The budget [`Expander::new`](#expandernew) uses.

```rust
use macro_lang::Budget;

assert_eq!(Budget::DEFAULT.max_depth, 128);
assert_eq!(Budget::DEFAULT.max_expansions, 1 << 20);
assert_eq!(Budget::DEFAULT.max_tokens, 1 << 22);
assert_eq!(Budget::default(), Budget::DEFAULT);
```

### `Budget::with_max_depth`, `with_max_expansions`, `with_max_tokens`

```rust,ignore
pub const fn with_max_depth(self, max_depth: u32) -> Budget
pub const fn with_max_expansions(self, max_expansions: usize) -> Budget
pub const fn with_max_tokens(self, max_tokens: usize) -> Budget
```

Return the budget with one field replaced. `const`, so a budget can be a
constant.

```rust
use macro_lang::Budget;

const EDITOR: Budget = Budget::DEFAULT.with_max_expansions(10_000).with_max_tokens(100_000);
assert_eq!(EDITOR.max_depth, 128);
assert_eq!(EDITOR.max_expansions, 10_000);
```

---

## `Limit`

```rust,ignore
#[non_exhaustive]
pub enum Limit {
    Expansions { max: usize },
    Tokens { max: usize },
}
```

*Added in 1.1.0.* Which part of a [`Budget`](#budget) an expansion ran out of,
carried by [`ExpandError::Budget`](#expanderror). `max` is the limit that was
reached. Depth is not listed: exceeding `max_depth` keeps reporting
`ExpandError::RecursionLimit`, as it did before budgets existed.
`#[non_exhaustive]`: further limits may be added in a minor release.

**Trait implementations:** `Clone`, `Copy`, `Debug`, `PartialEq`, `Eq`, `Hash`,
`Display` (`"<max> expansions"`, `"<max> output tokens"`).

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
// Three more tokens would make six.
assert_eq!(expander.expand(&three, &[], s, Context::ROOT),
           Err(ExpandError::Budget(Limit::Tokens { max: 5 })));
# Ok::<(), Box<dyn std::error::Error>>(())
```

---

## `Usage`

```rust,ignore
#[non_exhaustive]
pub struct Usage {
    pub expansions: usize,
    pub tokens: usize,
}
```

*Added in 1.1.0.* The work an [`Expander`](#expander) has charged against its
[`Budget`](#budget) since it was created or
[`reset_usage`](#expanderreset_usage) was last called: successful expansions
(compared against `max_expansions`) and the tokens they wrote (compared against
`max_tokens`, counted the same way). Returned by
[`Expander::usage`](#expanderusage). `Usage::NONE` is both counters zero.
`#[non_exhaustive]`: further counters may be added in a minor release.

**Trait implementations:** `Clone`, `Copy`, `Debug`, `Default` (=
`Usage::NONE`), `PartialEq`, `Eq`, `Hash`.

```rust
use macro_lang::{Expander, Usage};

let expander: Expander<char> = Expander::new();
assert_eq!(expander.usage(), Usage::NONE);
assert_eq!(Usage::default(), Usage::NONE);
```

---

## `Context`

```rust,ignore
pub struct Context(/* private */);

impl Context {
    pub const ROOT: Context;
    pub const fn is_root(self) -> bool;
    pub const fn as_u32(self) -> u32;
}
```

The hygiene context of a token: which expansion, if any, introduced it. A small
copyable handle. Two identifiers with the same spelling denote the same binding
only if their contexts are equal too.

**Items**

- `ROOT` — the context of tokens written directly in source. Also the `Default`.
- `is_root()` — `true` for `ROOT`.
- `as_u32()` — the raw index. Contexts are numbered densely from `0` in the
  order an expander mints them, so the index works as a key into a side table.

Contexts are only meaningful to the expander that minted them.

**Trait implementations:** `Clone`, `Copy`, `Debug`, `Default`, `PartialEq`,
`Eq`, `PartialOrd`, `Ord`, `Hash`, `Display` (formats as `#n`, the notation
rustc uses).

```rust
use macro_lang::Context;

assert!(Context::ROOT.is_root());
assert_eq!(Context::default(), Context::ROOT);
assert_eq!(Context::ROOT.as_u32(), 0);
assert_eq!(Context::ROOT.to_string(), "#0");
```

Every expansion mints its own context, so two expansions of the same macro never
share one:

```rust
use intern_lang::Interner;
use macro_lang::{Context, Expander, Macro, Rule, Template, Tree};
use token_lang::Span;

let mut names = Interner::new();
let s = Span::new(0, 1);
let mac = Macro::new(names.intern("t"), vec![Rule { pattern: vec![], template: vec![Template::token('t', s)] }])?;

let mut expander = Expander::new();
let ctx_of = |out: &[Tree<char>]| match out.first() {
    Some(Tree::Token { ctx, .. }) => *ctx,
    _ => Context::ROOT,
};
let a = ctx_of(&expander.expand(&mac, &[], s, Context::ROOT)?);
let b = ctx_of(&expander.expand(&mac, &[], s, Context::ROOT)?);
assert_ne!(a, b);
assert!(!a.is_root() && !b.is_root());
# Ok::<(), Box<dyn std::error::Error>>(())
```

---

## `Origin`

```rust,ignore
#[non_exhaustive]
pub struct Origin {
    pub parent: Context,
    pub macro_name: Symbol,
    pub call_site: Span,
    pub call_context: Context,
}
```

Where a minted [`Context`](#context) came from, returned by
[`Expander::origin`](#expanderorigin).

**Fields**

- `parent` — the context the token had in the macro definition, before this
  expansion marked it. `ROOT` for a macro defined in source. A resolver falls
  back to it for a macro-introduced name the expansion does not bind (a call to a
  global function from inside a template), so the name resolves where the macro
  was defined.
- `macro_name` — the macro whose expansion introduced the token.
- `call_site` — the span of the invocation that triggered the expansion.
- `call_context` — the context of the invocation itself. Following it outward
  walks the chain of nested expansions.

`#[non_exhaustive]`: only the expander builds an `Origin`, and fields may be
added in a minor release. Read the fields directly, or destructure with `..`.

**Trait implementations:** `Clone`, `Copy`, `Debug`, `PartialEq`, `Eq`, `Hash`.

A macro defined by another macro's expansion carries the definition context in
its template tokens, and `parent` reports it:

```rust
use intern_lang::Interner;
use macro_lang::{Context, Expander, Macro, Rule, Template, Tree};
use token_lang::Span;

let mut names = Interner::new();
let s = Span::new(0, 1);
let mut expander = Expander::new();

// An outer expansion produces a token `d` with a minted context...
let outer = Macro::new(names.intern("outer"), vec![Rule { pattern: vec![], template: vec![Template::token('d', s)] }])?;
let produced = expander.expand(&outer, &[], s, Context::ROOT)?;
let Some(Tree::Token { token, ctx: def_ctx }) = produced.first() else {
    return Err("expected one token".into());
};

// ...and a macro whose template is built from that output keeps the context.
let inner = Macro::new(names.intern("inner"), vec![Rule {
    pattern: vec![],
    template: vec![Template::Token { token: *token, ctx: *def_ctx }],
}])?;
let out = expander.expand(&inner, &[], s, Context::ROOT)?;
let Some(Tree::Token { ctx, .. }) = out.first() else {
    return Err("expected one token".into());
};
assert_eq!(expander.origin(*ctx).ok_or("unknown context")?.parent, *def_ctx);
# Ok::<(), Box<dyn std::error::Error>>(())
```

---

## `MacroError`

```rust,ignore
#[non_exhaustive]
pub enum MacroError {
    NoRules,
    DuplicateBinding { rule: usize, name: Symbol },
    UnboundVariable { rule: usize, name: Symbol },
    StillRepeating { rule: usize, name: Symbol },
    NoRepeatingVariable { rule: usize },
    EmptyRepetition { rule: usize },
    OptionalSeparator { rule: usize },
}
```

Why [`Macro::new`](#macronew) rejected a definition. `rule` is the index of the
offending rule in the list passed to `Macro::new`; `name` is the metavariable's
`Symbol`, which the caller resolves through its interner. The enum is
`#[non_exhaustive]`: later releases may add checks.

| Variant | Meaning | Fix |
|---|---|---|
| `NoRules` | The macro has no rules. | Give it at least one rule. |
| `DuplicateBinding` | A pattern binds `name` twice. | Rename one binding. |
| `UnboundVariable` | A template uses `name`, which the pattern never binds. | Bind it, or write the token literally. |
| `StillRepeating` | `name` repeats in the pattern but is used outside enough template repetitions. | Wrap the use in a `Template::Repeat`. |
| `NoRepeatingVariable` | A template repetition contains no metavariable repeating at its depth. | Use a repeating metavariable inside, or remove the repetition. |
| `EmptyRepetition` | A pattern repetition can match without consuming input. | Give the body a token, group, or binding that must be present. |
| `OptionalSeparator` | A `?` repetition has a separator. | Remove the separator. |

**Trait implementations:** `Clone`, `Copy`, `Debug`, `PartialEq`, `Eq`, `Hash`,
`Display`, `core::error::Error`.

```rust
use intern_lang::Interner;
use macro_lang::{Fragment, Kleene, Macro, MacroError, Pattern, Rule, Template};

let mut names = Interner::new();
let (x, m) = (names.intern("x"), names.intern("m"));
let bind = |name| Pattern::Bind { name, fragment: Fragment::Tree };

let unbound = Macro::<char>::new(m, vec![Rule { pattern: vec![], template: vec![Template::Var(x)] }]);
assert_eq!(unbound.unwrap_err(), MacroError::UnboundVariable { rule: 0, name: x });

let duplicate = Macro::<char>::new(m, vec![Rule { pattern: vec![bind(x), bind(x)], template: vec![] }]);
assert_eq!(duplicate.unwrap_err(), MacroError::DuplicateBinding { rule: 0, name: x });

let empty = Macro::<char>::new(m, vec![Rule {
    pattern: vec![Pattern::Repeat { body: vec![], separator: None, kleene: Kleene::ZeroOrMore }],
    template: vec![],
}]);
assert_eq!(empty.unwrap_err(), MacroError::EmptyRepetition { rule: 0 });
```

Rendering a definition error with the metavariable's name:

```rust
use intern_lang::Interner;
use macro_lang::{Macro, MacroError, Rule, Template};

let mut names = Interner::new();
let x = names.intern("value");
let err = Macro::<char>::new(names.intern("m"), vec![Rule { pattern: vec![], template: vec![Template::Var(x)] }])
    .unwrap_err();

let message = match err {
    MacroError::UnboundVariable { rule, name } => format!(
        "rule {rule}: `${}` is not bound by the pattern",
        names.resolve(name).unwrap_or("?")
    ),
    other => other.to_string(),
};
assert_eq!(message, "rule 0: `$value` is not bound by the pattern");
```

---

## `ExpandError`

```rust,ignore
#[non_exhaustive]
pub enum ExpandError {
    NoMatch { span: Span },
    Ambiguous { rule: usize },
    RepetitionMismatch { rule: usize },
    RecursionLimit { limit: u32 },
    Budget(Limit), // 1.1.0
    ContextOverflow,
}
```

Why [`Expander::expand`](#expanderexpand) could not expand an invocation. A failed
expansion leaves the expander unchanged. The enum is `#[non_exhaustive]`.

| Variant | Meaning | What to do |
|---|---|---|
| `NoMatch { span }` | No rule matched. `span` is the token the furthest-reaching rule could not get past, or an empty span just past the input if every rule wanted more (the `call_site` for an empty invocation). | Report "no rules expected this token" at `span`. |
| `Ambiguous { rule }` | Rule `rule` matched in more than one way. Rules after it are not tried. | Make the rule unambiguous, typically with a separator or literal between repetitions. |
| `RepetitionMismatch { rule }` | Lockstep metavariables captured different numbers of trees. | Match them in one pattern repetition, or expand them separately. |
| `RecursionLimit { limit }` | Nesting exceeded the limit (`Budget::max_depth`). | Fix a macro with no base case, or raise the limit. |
| `Budget(limit)` | *Added in 1.1.0.* The expander ran out of expansions or tokens; [`limit`](#limit) says which, and its value. | Fix a macro whose output grows without bound (one that doubles its input, or expands into several calls of itself), or raise the budget with [`set_budget`](#expanderset_budget). A long-lived host that never calls [`reset_usage`](#expanderreset_usage) eventually hits this on legitimate input: reset per unit of work. |
| `ContextOverflow` | All `u32::MAX` contexts are in use. | Start a fresh expander. |

**Trait implementations:** `Clone`, `Copy`, `Debug`, `PartialEq`, `Eq`, `Hash`,
`Display`, `core::error::Error`.

`NoMatch` pointing at the offending token:

```rust
use intern_lang::Interner;
use macro_lang::{Context, ExpandError, Expander, Macro, Pattern, Rule};
use token_lang::Span;
# use macro_lang::Tree;
# fn trees(src: &str) -> Vec<Tree<char>> {
#     src.char_indices().map(|(i, c)| Tree::token(c, Span::new(i as u32, i as u32 + 1))).collect()
# }

let mut names = Interner::new();
let abc = Macro::new(names.intern("abc"), vec![Rule {
    pattern: vec![Pattern::Token('a'), Pattern::Token('b'), Pattern::Token('c')],
    template: vec![],
}])?;

let mut expander = Expander::new();
let call = Span::new(0, 20);
// Stuck at `x`, the third token.
assert_eq!(expander.expand(&abc, &trees("abx"), call, Context::ROOT),
           Err(ExpandError::NoMatch { span: Span::new(2, 3) }));
// The input ended early: an empty span just past `b`.
assert_eq!(expander.expand(&abc, &trees("ab"), call, Context::ROOT),
           Err(ExpandError::NoMatch { span: Span::empty(2) }));
# Ok::<(), Box<dyn std::error::Error>>(())
```

`Ambiguous` and `RepetitionMismatch`:

```rust
use intern_lang::Interner;
use macro_lang::{Context, ExpandError, Expander, Fragment, Kleene, Macro, Pattern, Rule, Template};
use token_lang::Span;
# use macro_lang::Tree;
# fn trees(src: &str) -> Vec<Tree<char>> {
#     src.char_indices().map(|(i, c)| Tree::token(c, Span::new(i as u32, i as u32 + 1))).collect()
# }

let mut names = Interner::new();
let (a, b) = (names.intern("a"), names.intern("b"));
let star = |name| Pattern::Repeat {
    body: vec![Pattern::Bind { name, fragment: Fragment::Tree }],
    separator: None,
    kleene: Kleene::ZeroOrMore,
};
let s = Span::new(0, 1);
let mut expander = Expander::new();

// $($a:tt)* $($b:tt)* — where does `a` end?
let split = Macro::new(names.intern("split"), vec![Rule { pattern: vec![star(a), star(b)], template: vec![] }])?;
assert_eq!(expander.expand(&split, &trees("xy"), s, Context::ROOT),
           Err(ExpandError::Ambiguous { rule: 0 }));

// $($a:tt)* ; $($b:tt)*  =>  $( $a $b )* — needs equal counts.
let zip = Macro::new(names.intern("zip"), vec![Rule {
    pattern: vec![star(a), Pattern::Token(';'), star(b)],
    template: vec![Template::Repeat { body: vec![Template::Var(a), Template::Var(b)], separator: None }],
}])?;
assert!(expander.expand(&zip, &trees("ab;cd"), s, Context::ROOT).is_ok());
assert_eq!(expander.expand(&zip, &trees("ab;c"), s, Context::ROOT),
           Err(ExpandError::RepetitionMismatch { rule: 0 }));
# Ok::<(), Box<dyn std::error::Error>>(())
```

---

## Feature flags

| Feature | Default | Effect |
|---|---|---|
| `std` | yes | Links the standard library. Without it the crate is `#![no_std]` and needs only `alloc`; behaviour is identical. |

```toml
[dependencies]
macro-lang = { version = "1", default-features = false }   # no_std + alloc
```

---

## Guide: writing a front end

macro-lang does the matching, substitution, and hygiene; a language supplies the
four pieces around it. The [`examples/common`](../examples/common/mod.rs) module
implements all four for a small toy language and is the recommended starting
point.

1. **Lex** source into `token_lang::Token<Kind>`. Make `Kind` small and `Copy`;
   carry identifiers and literals as interned `Symbol`s.
2. **Group** the token stream into [`Tree`](#tree)s by matching delimiters,
   giving every token [`Context::ROOT`](#context) (see the
   [grouping example](#tree)).
3. **Lower macro definitions.** Parse your language's macro syntax — for example
   `($a:ident, $b:ident) => { ... }` — into [`Pattern`](#pattern)s and
   [`Template`](#template)s, mapping fragment specifiers to
   [`Fragment`](#fragment)s (`tt` → `Fragment::Tree`, `ident` →
   `Fragment::Kind(Kind::is_ident)`), and build the [`Macro`](#macro). When a
   definition itself came out of an expansion, copy each token's context into
   `Template::Token { ctx, .. }` — and each repetition separator's into
   `Template::RepeatSeparated { ctx, .. }` — so nested hygiene is preserved.
4. **Drive expansion.** Walk the trees; at each invocation (say, an identifier
   naming a macro, `!`, and a group), resolve the macro, call
   [`expand`](#expanderexpand) with the group's contents, the invocation span,
   and the **name token's context**, splice the output in place of the
   invocation, and rescan from the same position so invocations in the output
   are expanded too. Stop when none remain. A driver that instead expands each
   output before splicing it knows how deep every invocation sits; it should
   call [`expand_at`](#expanderexpand_at) with that depth (see
   [bounding untrusted macros](#guide-bounding-untrusted-macros)).

A compact driver for a single macro, invoked as `!(...)`:

```rust
use intern_lang::Interner;
use macro_lang::{Context, ExpandError, Expander, Fragment, Kleene, Macro, Pattern, Rule, Template, Tree};
use token_lang::{Span, Token};

/// Expands every `!( ... )` invocation of `mac` in `stream` to a fixed point.
fn drive(expander: &mut Expander<char>, mac: &Macro<char>, stream: &mut Vec<Tree<char>>) -> Result<(), ExpandError> {
    let mut i = 0;
    while i + 1 < stream.len() {
        let call = match (&stream[i], &stream[i + 1]) {
            (Tree::Token { token, ctx }, Tree::Group { trees, close, .. }) if token.kind == '!' => {
                Some((trees.clone(), token.span.merge(close.span), *ctx))
            }
            _ => None,
        };
        match call {
            Some((args, span, ctx)) => {
                let out = expander.expand(mac, &args, span, ctx)?;
                let _replaced: Vec<_> = stream.splice(i..i + 2, out).collect();
            }
            None => i += 1,
        }
    }
    Ok(())
}

// count!()               => 0
// count!($x $($rest)*)   => 1 + count!($($rest)*)
let mut names = Interner::new();
let (x, rest) = (names.intern("x"), names.intern("rest"));
let s = Span::new(0, 1);
let count = Macro::new(names.intern("count"), vec![
    Rule { pattern: vec![], template: vec![Template::token('0', s)] },
    Rule {
        pattern: vec![
            Pattern::Bind { name: x, fragment: Fragment::Tree },
            Pattern::Repeat {
                body: vec![Pattern::Bind { name: rest, fragment: Fragment::Tree }],
                separator: None,
                kleene: Kleene::ZeroOrMore,
            },
        ],
        template: vec![
            Template::token('1', s),
            Template::token('+', s),
            Template::token('!', s),
            Template::Group {
                open: Token::new('(', s),
                close: Token::new(')', s),
                body: vec![Template::Repeat { body: vec![Template::Var(rest)], separator: None }],
            },
        ],
    },
])?;

let mut stream = vec![
    Tree::token('!', s),
    Tree::Group {
        open: Token::new('(', s),
        close: Token::new(')', s),
        trees: vec![Tree::token('a', s), Tree::token('b', s)],
    },
];
drive(&mut Expander::new(), &count, &mut stream)?;
let text: String = stream
    .iter()
    .filter_map(|t| match t { Tree::Token { token, .. } => Some(token.kind), _ => None })
    .collect();
assert_eq!(text, "1+1+0");
# Ok::<(), Box<dyn std::error::Error>>(())
```

---

## Guide: resolving names with contexts

Hygiene is enforced by the name resolver, using the contexts macro-lang records.
The rule is short:

1. **Bindings are keyed by `(Symbol, Context)`.** A `let tmp` written by a
   template binds `(tmp, #3)`; the caller's `let tmp` binds `(tmp, #0)`. They
   never collide.
2. **References look up their own `(Symbol, Context)` first.** A `tmp` written by
   the same expansion finds the template's binding; the caller's `tmp` finds the
   caller's.
3. **Free macro-introduced names fall back to the definition site.** If a
   reference `(name, ctx)` finds no binding and `ctx` is not root, retry with
   `expander.origin(ctx)?.parent` — the context the token had where the macro was
   defined. That is how a template can call a global function by name.

```rust
use std::collections::HashMap;

use intern_lang::{Interner, Symbol};
use macro_lang::{Context, Expander, Fragment, Macro, Pattern, Rule, Template, Tree};
use token_lang::Span;

/// Resolves `(name, ctx)` against `scope`, falling back to definition sites.
fn resolve(
    scope: &HashMap<(Symbol, Context), &'static str>,
    expander: &Expander<char>,
    name: Symbol,
    mut ctx: Context,
) -> Option<&'static str> {
    loop {
        if let Some(found) = scope.get(&(name, ctx)) {
            return Some(found);
        }
        ctx = expander.origin(ctx)?.parent;
    }
}

let mut names = Interner::new();
let tmp = names.intern("tmp");
let x = names.intern("x");

// introduce!($x:tt) => t $x — the template writes its own `t`.
let introduce = Macro::new(names.intern("introduce"), vec![Rule {
    pattern: vec![Pattern::Bind { name: x, fragment: Fragment::Tree }],
    template: vec![Template::token('t', Span::new(0, 1)), Template::Var(x)],
}])?;
let mut expander = Expander::new();
let out = expander.expand(&introduce, &[Tree::token('t', Span::new(5, 6))], Span::new(0, 7), Context::ROOT)?;
let ctx = |i: usize| match &out[i] { Tree::Token { ctx, .. } => *ctx, _ => Context::ROOT };
let (macro_t, caller_t) = (ctx(0), ctx(1));

// Suppose the template's `t` declared `tmp` and the caller's `t` is a use of
// the caller's own `tmp`. Each resolves to its own binding.
let mut scope = HashMap::new();
scope.insert((tmp, macro_t), "the macro's temporary");
scope.insert((tmp, Context::ROOT), "the caller's variable");
assert_eq!(resolve(&scope, &expander, tmp, macro_t), Some("the macro's temporary"));
assert_eq!(resolve(&scope, &expander, tmp, caller_t), Some("the caller's variable"));

// A name the template uses but never binds falls back to the definition site.
let global = names.intern("print");
scope.insert((global, Context::ROOT), "the global function");
assert_eq!(resolve(&scope, &expander, global, macro_t), Some("the global function"));
# Ok::<(), Box<dyn std::error::Error>>(())
```

---

## Guide: bounding untrusted macros

Every expander is bounded by default — no configuration is needed for a macro
to be unable to hang the compiler or exhaust memory. What a driver can add is
*precision*: which error a runaway macro gets, and how soon.

- **Track depth if you can.** A driver that recurses into each expansion's
  output before splicing it (the shape of rustc's expansion collector) knows
  the depth of the code it is scanning. Passing it to
  [`expand_at`](#expanderexpand_at) makes the recursion limit hold for every
  chain of nested expansions, including a macro that rebuilds its own
  invocation from captured tokens (`m!($a:tt $b:tt) => $a $b ($a $b)` invoked
  as `m!(m !)`). [`examples/common`](../examples/common/mod.rs) has such a
  driver, `Driver::expand_tracked`.
- **Rescanning drivers rely on hygiene and the budget.** A driver that splices
  and rescans the whole stream only has the tokens to go on. Depth is then
  inferred from the contexts of the name token and the input, which catches
  every macro that writes any literal token into the call it builds. A call
  rebuilt only from captured source tokens is indistinguishable from the
  original, and the expansion and token budgets stop it instead, in bounded
  time — about 0.15 s with the defaults on the benchmark machine.
- **Reset per unit of work in a long-lived host.** Usage accumulates until
  [`reset_usage`](#expanderreset_usage). A batch compiler never needs it; a
  language server, REPL, or watch-mode compiler that keeps one expander calls
  `reset_usage()` before each document parse, REPL entry, or rebuild, so the
  budget bounds each unit of work, and one hostile document cannot use up the
  budget of the documents after it. Contexts and origins survive the reset.
  `examples/budget.rs` shows the loop.
- **Size the budget to the environment.** The defaults suit a batch compiler.
  An editor that expands on every keystroke wants tighter limits:

```rust
use macro_lang::{Budget, Expander};

const INTERACTIVE: Budget = Budget::DEFAULT
    .with_max_depth(64)
    .with_max_expansions(20_000)
    .with_max_tokens(500_000);
let expander: Expander<char> = Expander::with_budget(INTERACTIVE);
assert_eq!(expander.limit(), 64);
```

A doubling macro under the default budget — it reaches the token limit in about
twenty rounds:

```rust
use intern_lang::Interner;
use macro_lang::{Budget, Context, ExpandError, Expander, Fragment, Kleene, Limit, Macro, Pattern, Rule, Template, Tree};
use token_lang::{Span, Token};

let mut names = Interner::new();
let t = names.intern("t");
let s = Span::new(0, 1);
let each = || Template::Repeat { body: vec![Template::Var(t)], separator: None };
// dup!($($t:tt)*) => dup!($($t)* $($t)*)
let dup = Macro::new(names.intern("dup"), vec![Rule {
    pattern: vec![Pattern::Repeat {
        body: vec![Pattern::Bind { name: t, fragment: Fragment::Tree }],
        separator: None,
        kleene: Kleene::ZeroOrMore,
    }],
    template: vec![
        Template::token('d', s),
        Template::token('!', s),
        Template::Group { open: Token::new('(', s), close: Token::new(')', s), body: vec![each(), each()] },
    ],
}])?;

let mut expander = Expander::with_budget(Budget::DEFAULT.with_max_tokens(1 << 16));
let mut args = vec![Tree::token('x', s)];
let mut call_context = Context::ROOT;
let mut rounds = 0;
let err = loop {
    match expander.expand(&dup, &args, s, call_context) {
        Ok(mut out) => {
            if let Some(Tree::Token { ctx, .. }) = out.first() { call_context = *ctx; }
            match out.pop() {
                Some(Tree::Group { trees, .. }) => { args = trees; rounds += 1; }
                _ => break None,
            }
        }
        Err(err) => break Some(err),
    }
};
assert_eq!(err, Some(ExpandError::Budget(Limit::Tokens { max: 1 << 16 })));
assert!(rounds < 20);
# Ok::<(), Box<dyn std::error::Error>>(())
```

---

## Stability

As of `1.0.0` the public API is frozen. macro-lang follows
[Semantic Versioning](https://semver.org/); within the `1.x` series:

- **Added in 1.1.0** (additive, no breaking change): [`Budget`](#budget)
  (`DEFAULT`, `with_max_depth`, `with_max_expansions`, `with_max_tokens`),
  [`Limit`](#limit), `ExpandError::Budget`, `Template::RepeatSeparated`, and
  [`Usage`](#usage) (`NONE`), and `Expander::with_budget`, `budget`,
  `set_budget`, `usage`, `reset_usage`, and `expand_at`. These are
  part of the frozen surface from 1.1.0 on.

- The **surface** will not change in a breaking way: [`Tree`](#tree)
  (`token`, `span`), [`Pattern`](#pattern), [`Fragment`](#fragment),
  [`Kleene`](#kleene) (`allows`), [`Template`](#template) (`token`),
  [`Rule`](#rule), [`Macro`](#macro) (`new`, `name`),
  [`Expander`](#expander) (`DEFAULT_LIMIT`, `new`, `with_limit`, `limit`,
  `with_budget`, `budget`, `set_budget`, `usage`, `reset_usage`, `expand`,
  `expand_at`, `origin`), [`Usage`](#usage),
  [`Budget`](#budget), [`Limit`](#limit), [`Context`](#context) (`ROOT`, `is_root`, `as_u32`),
  [`Origin`](#origin), [`MacroError`](#macroerror), and
  [`ExpandError`](#expanderror). A breaking change means a new major version.
- The **matching semantics** are part of the contract: rules are tried in
  order and the first whose pattern matches the entire invocation wins; literal
  tokens compare by kind only; a rule that matches in more than one way is
  reported as `Ambiguous` and stops the search; matching never backtracks; and
  `NoMatch` locates the furthest point any rule reached.
- The **hygiene semantics** are part of the contract: each expansion mints
  fresh contexts for the literal tokens and separators its template writes —
  one context per distinct definition context — and substituted captures keep
  their contexts unchanged; `Origin` reports the definition context, macro,
  call site, and call context; a failed expansion leaves the expander
  unchanged. Contexts are numbered densely from `0` in the order they are
  minted. A `Repeat` separator is defined in the root context; a
  `RepeatSeparated` separator in its `ctx`.
- The **depth semantics**: an invocation runs one level deeper than the deepest
  of the driver's `depth` (`0` for `expand`), the expansion that produced
  `call_context`, and the expansions that produced its input tokens. *Changed
  in 1.1.0:* 1.0 read depth from `call_context` alone, which let a macro that
  rebuilt its invocation around a captured name restart at depth 1 every
  round. Depth can only be higher than 1.0 computed, never lower.
- The **budget semantics**: the expansion and token limits apply to the
  totals of successful expansions since creation or the last `reset_usage`,
  which zeroes only those counters (hygiene state, the budget, and depth are
  untouched); tokens are counted one per token, two per group,
  and every token of a substituted capture, and are charged before they are
  written; failed expansions are not charged.
- The **definition checks** in [`Macro::new`](#macronew) are fixed: a macro that
  builds under `1.0` keeps building under every `1.x`.
- [`Expander::DEFAULT_LIMIT`](#expanderdefault_limit) stays `128`. The
  [`Budget::DEFAULT`](#budgetdefault) expansion and token limits will not be
  lowered within `1.x` (raising them is a minor change).
- `Pattern`, `Fragment`, `Template`, `Origin`, `MacroError`, `ExpandError`,
  `Budget`, `Limit`, and `Usage` are `#[non_exhaustive]`, so new elements, fields, and failure modes are
  additive minor changes. Match them with a wildcard arm.
- MSRV (Rust 1.85) is a compatibility surface: raising it is a documented minor
  change, never a patch.

What is **not** promised: the exact `Display` wording of the error types, the
`Debug` output of any type, the compiled representation of a `Macro`, and
performance figures — the benchmarks are tracked, but they are measurements,
not guarantees.

See [`../dev/ROADMAP.md`](../dev/ROADMAP.md) and
[`../CHANGELOG.md`](../CHANGELOG.md).
