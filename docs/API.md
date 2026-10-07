# macro-lang &mdash; API Reference

> Complete reference for every public item in `macro-lang`, with examples.
> **Status: pre-1.0 (0.2).** The surface below is the core the 1.0 contract will
> be cut from; it may still change in a minor release before `1.0.0`. See
> [`../dev/ROADMAP.md`](../dev/ROADMAP.md).

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
  - [`Expander::expand`](#expanderexpand)
  - [`Expander::origin`](#expanderorigin)
- [`Context`](#context)
- [`Origin`](#origin)
- [`MacroError`](#macroerror)
- [`ExpandError`](#expanderror)
- [Feature flags](#feature-flags)
- [Guide: writing a front end](#guide-writing-a-front-end)
- [Guide: resolving names with contexts](#guide-resolving-names-with-contexts)
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
| [`Expander`](#expander) | Runs expansions; owns the hygiene table and the scratch buffers. |
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
macro-lang = "0.2"
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
pub enum Template<K> {
    Token { token: Token<K>, ctx: Context },
    Group { open: Token<K>, close: Token<K>, body: Vec<Template<K>> },
    Var(Symbol),
    Repeat { body: Vec<Template<K>>, separator: Option<Token<K>> },
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
  token defined in the root context. The iteration count comes from the
  metavariables used inside (see [lockstep](#repetition-depth-and-lockstep)).

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
that minted it — and **pooled scratch buffers** reused by every call, so
steady-state expansion allocates only the output trees.

Use one expander for a whole compilation (or one per thread), so every context in
the program comes from one table. The expander does not find invocations or look
macros up by name; that is the expansion driver's job (see
[writing a front end](#guide-writing-a-front-end)).

**Trait implementations:** `Default` (same as [`new`](#expandernew)), `Debug`
(prints the limit and the number of contexts and expansions recorded), `Send` and
`Sync` (where `K` is).

### `Expander::DEFAULT_LIMIT`

```rust,ignore
pub const DEFAULT_LIMIT: u32 = 128;
```

The recursion limit [`Expander::new`](#expandernew) uses: 128 nested expansions,
the same default as rustc's `recursion_limit`.

```rust
use macro_lang::Expander;

assert_eq!(Expander::<char>::DEFAULT_LIMIT, 128);
```

### `Expander::new`

```rust,ignore
pub fn new() -> Expander<K>
```

Creates an expander with the default recursion limit and an empty hygiene table.

```rust
use macro_lang::Expander;

let expander: Expander<char> = Expander::new();
assert_eq!(expander.limit(), 128);
assert_eq!(format!("{expander:?}"), "Expander { limit: 128, contexts: 0, expansions: 0 }");
```

### `Expander::with_limit`

```rust,ignore
pub fn with_limit(limit: u32) -> Expander<K>
```

Creates an expander that allows invocations nested at most `limit` expansions
deep.

**Parameters**

- `limit` — the maximum nesting depth. Depth is read from hygiene: an invocation
  whose `call_context` was minted by an expansion `n` levels deep runs at level
  `n + 1`. A limit of `0` rejects every expansion.

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

Returns the recursion limit.

```rust
use macro_lang::Expander;

assert_eq!(Expander::<char>::with_limit(16).limit(), 16);
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

**Returns** the expanded trees.

**Errors**

| Error | Cause |
|---|---|
| [`RecursionLimit`](#expanderror) | The invocation is nested deeper than the [limit](#expanderlimit). |
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
| `RecursionLimit { limit }` | Nesting exceeded the limit. | Fix a macro with no base case, or raise the limit. |
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
macro-lang = { version = "0.2", default-features = false }   # no_std + alloc
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
   `Template::Token { ctx, .. }` so nested hygiene is preserved.
4. **Drive expansion.** Walk the trees; at each invocation (say, an identifier
   naming a macro, `!`, and a group), resolve the macro, call
   [`expand`](#expanderexpand) with the group's contents, the invocation span,
   and the **name token's context**, splice the output in place of the
   invocation, and rescan from the same position so invocations in the output
   are expanded too. Stop when none remain.

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

## Stability

macro-lang is pre-1.0. The surface documented here is the core the `1.0`
contract will be cut from, and the roadmap's next milestone freezes it; until
then a minor release may still change it. The error enums are
`#[non_exhaustive]`, so new failure modes can be added without breaking a
`match`. MSRV is Rust 1.85 and is treated as part of the compatibility surface.
