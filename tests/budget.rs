//! Termination and resource bounds: the recursion limit against macros that
//! rebuild their own invocation, the expansion budget against expansion bombs,
//! and the hygiene of repetition separators.
//!
//! Tokens are `char`s. A macro is invoked as `<name> ! ( args )`, where
//! `<name>` is a single-character token registered with a driver. Two drivers
//! are used, matching the two ways a real front end can be written:
//!
//! - [`rescan`] splices each expansion back into the stream and searches the
//!   whole stream again, passing only the name token's context — the
//!   [`Expander::expand`] contract, where nesting depth comes from hygiene;
//! - [`tracked`] expands each output before splicing it, so it knows how deep
//!   every invocation sits and passes that to [`Expander::expand_at`].

#![allow(clippy::unwrap_used)]

use std::collections::HashMap;
use std::time::{Duration, Instant};

use intern_lang::{Interner, Symbol};
use macro_lang::{
    Budget, Context, ExpandError, Expander, Fragment, Kleene, Limit, Macro, Pattern, Rule,
    Template, Tree, Usage,
};
use proptest::prelude::*;
use token_lang::{Span, Token};

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

fn sp() -> Span {
    Span::new(500, 501)
}

fn closer(open: char) -> Option<char> {
    match open {
        '(' => Some(')'),
        '[' => Some(']'),
        '{' => Some('}'),
        _ => None,
    }
}

/// Builds token trees from `src`; each token's span is its byte offset.
fn trees(src: &str) -> Vec<Tree<char>> {
    let mut stack: Vec<(Token<char>, Vec<Tree<char>>)> = Vec::new();
    let mut top = Vec::new();
    for (i, c) in src.char_indices() {
        let span = Span::new(i as u32, i as u32 + 1);
        if c.is_whitespace() {
            continue;
        }
        if closer(c).is_some() {
            stack.push((Token::new(c, span), std::mem::take(&mut top)));
        } else if matches!(c, ')' | ']' | '}') {
            let (open, parent) = stack.pop().unwrap();
            let inner = std::mem::replace(&mut top, parent);
            top.push(Tree::Group {
                open,
                close: Token::new(c, span),
                trees: inner,
            });
        } else {
            top.push(Tree::token(c, span));
        }
    }
    assert!(stack.is_empty(), "unbalanced test input");
    top
}

fn render(trees: &[Tree<char>]) -> String {
    let mut out = String::new();
    for tree in trees {
        match tree {
            Tree::Token { token, .. } => out.push(token.kind),
            Tree::Group { open, close, trees } => {
                out.push(open.kind);
                out.push_str(&render(trees));
                out.push(close.kind);
            }
        }
    }
    out
}

/// Tokens in `trees`, counting both delimiters of every group.
fn token_count(trees: &[Tree<char>]) -> usize {
    trees
        .iter()
        .map(|tree| match tree {
            Tree::Token { .. } => 1,
            Tree::Group { trees, .. } => 2 + token_count(trees),
        })
        .sum()
}

/// Every token's kind and context, depth first.
fn contexts(trees: &[Tree<char>]) -> Vec<(char, Context)> {
    let mut out = Vec::new();
    for tree in trees {
        match tree {
            Tree::Token { token, ctx } => out.push((token.kind, *ctx)),
            Tree::Group { trees, .. } => out.extend(contexts(trees)),
        }
    }
    out
}

fn lit(c: char) -> Template<char> {
    Template::token(c, sp())
}

fn tgroup(open: char, body: Vec<Template<char>>) -> Template<char> {
    Template::Group {
        open: Token::new(open, sp()),
        close: Token::new(closer(open).unwrap(), sp()),
        body,
    }
}

fn tt(name: Symbol) -> Pattern<char> {
    Pattern::Bind {
        name,
        fragment: Fragment::Tree,
    }
}

fn star(name: Symbol) -> Pattern<char> {
    Pattern::Repeat {
        body: vec![tt(name)],
        separator: None,
        kleene: Kleene::ZeroOrMore,
    }
}

fn trep(body: Vec<Template<char>>) -> Template<char> {
    Template::Repeat {
        body,
        separator: None,
    }
}

/// A recognized invocation: the macro, the name token's context, the call
/// site, and the argument trees.
type Call<'m, 't> = (&'m Macro<char>, Context, Span, &'t [Tree<char>]);

/// The macros a driver knows, keyed by their one-character name.
#[derive(Default)]
struct Macros(HashMap<char, Macro<char>>);

impl Macros {
    fn with(mut self, name: char, mac: Macro<char>) -> Self {
        let _previous = self.0.insert(name, mac);
        self
    }

    /// Recognizes `name ! ( args )` at the start of `trees`.
    fn invocation<'t>(&self, trees: &'t [Tree<char>]) -> Option<Call<'_, 't>> {
        let [
            Tree::Token { token, ctx },
            Tree::Token { token: bang, .. },
            Tree::Group {
                open,
                trees: args,
                close,
            },
            ..,
        ] = trees
        else {
            return None;
        };
        if bang.kind != '!' || open.kind != '(' {
            return None;
        }
        let mac = self.0.get(&token.kind)?;
        Some((mac, *ctx, token.span.merge(close.span), args))
    }
}

/// Splices expansions back in and rescans from the start until no invocation
/// is left anywhere in the stream; nesting depth comes from hygiene alone.
fn rescan(
    expander: &mut Expander<char>,
    macros: &Macros,
    src: &str,
) -> Result<Vec<Tree<char>>, ExpandError> {
    let mut stream = trees(src);
    while expand_first(expander, macros, &mut stream)? {}
    Ok(stream)
}

/// Expands the first invocation in `trees` (searching into groups) in place.
fn expand_first(
    expander: &mut Expander<char>,
    macros: &Macros,
    trees: &mut Vec<Tree<char>>,
) -> Result<bool, ExpandError> {
    for i in 0..trees.len() {
        if let Some((mac, ctx, span, args)) = macros.invocation(&trees[i..]) {
            let out = expander.expand(mac, args, span, ctx)?;
            let _invocation: Vec<_> = trees.splice(i..i + 3, out).collect();
            return Ok(true);
        }
        if let Tree::Group { trees: inner, .. } = &mut trees[i] {
            if expand_first(expander, macros, inner)? {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

/// Expands every invocation in `trees`, found in code `depth` expansions deep,
/// expanding each output fully before splicing it. Records the deepest level
/// any expansion ran at in `deepest`.
fn tracked(
    expander: &mut Expander<char>,
    macros: &Macros,
    trees: Vec<Tree<char>>,
    depth: u32,
    deepest: &mut u32,
) -> Result<Vec<Tree<char>>, ExpandError> {
    let mut out = Vec::with_capacity(trees.len());
    let mut i = 0;
    while i < trees.len() {
        if let Some((mac, ctx, span, args)) = macros.invocation(&trees[i..]) {
            let expanded = expander.expand_at(mac, args, span, ctx, depth)?;
            *deepest = (*deepest).max(depth + 1);
            out.extend(tracked(expander, macros, expanded, depth + 1, deepest)?);
            i += 3;
            continue;
        }
        match &trees[i] {
            Tree::Group {
                open,
                close,
                trees: inner,
            } => out.push(Tree::Group {
                open: *open,
                close: *close,
                trees: tracked(expander, macros, inner.clone(), depth, deepest)?,
            }),
            token => out.push(token.clone()),
        }
        i += 1;
    }
    Ok(out)
}

fn run_tracked(
    expander: &mut Expander<char>,
    macros: &Macros,
    src: &str,
) -> (Result<Vec<Tree<char>>, ExpandError>, u32) {
    let mut deepest = 0;
    let result = tracked(expander, macros, trees(src), 0, &mut deepest);
    (result, deepest)
}

/// `$a:tt $b:tt => $a $b ($a $b)` — the reported H07 macro. Invoked as
/// `m!(m !)` it writes `m ! (m !)`: a call identical to the one it came from,
/// made only of tokens captured from source.
fn quine(names: &mut Interner) -> Macro<char> {
    let (a, b) = (names.intern("a"), names.intern("b"));
    Macro::new(
        names.intern("m"),
        vec![Rule {
            pattern: vec![tt(a), tt(b)],
            template: vec![
                Template::Var(a),
                Template::Var(b),
                tgroup('(', vec![Template::Var(a), Template::Var(b)]),
            ],
        }],
    )
    .unwrap()
}

/// `dup!($($t:tt)*) => dup!($($t)* $($t)*)` — doubles its argument each round.
fn dup(names: &mut Interner, name: char) -> Macro<char> {
    let t = names.intern("t");
    Macro::new(
        names.intern("dup"),
        vec![Rule {
            pattern: vec![star(t)],
            template: vec![
                lit(name),
                lit('!'),
                tgroup(
                    '(',
                    vec![trep(vec![Template::Var(t)]), trep(vec![Template::Var(t)])],
                ),
            ],
        }],
    )
    .unwrap()
}

// ---------------------------------------------------------------------------
// H07: the recursion limit holds for macros that rebuild their own invocation
// ---------------------------------------------------------------------------

#[test]
fn reported_quine_hits_the_recursion_limit_with_a_tracking_driver() {
    let mut names = Interner::new();
    let macros = Macros::default().with('m', quine(&mut names));
    let mut expander = Expander::with_limit(16);
    let (result, deepest) = run_tracked(&mut expander, &macros, "m!(m !)");
    assert_eq!(result, Err(ExpandError::RecursionLimit { limit: 16 }));
    assert_eq!(
        deepest, 16,
        "sixteen levels ran before the seventeenth was refused"
    );
}

#[test]
fn reported_quine_hits_the_default_limit() {
    let mut names = Interner::new();
    let macros = Macros::default().with('m', quine(&mut names));
    let mut expander = Expander::new();
    let (result, deepest) = run_tracked(&mut expander, &macros, "m!(m !)");
    assert_eq!(
        result,
        Err(ExpandError::RecursionLimit {
            limit: Expander::<char>::DEFAULT_LIMIT
        })
    );
    assert_eq!(deepest, Expander::<char>::DEFAULT_LIMIT);
}

#[test]
fn reported_quine_is_stopped_by_the_expansion_budget_under_plain_expand() {
    // Through `expand` alone the rebuilt call is indistinguishable from the
    // original, so depth stays 1; the expansion budget is what ends it.
    let mut names = Interner::new();
    let macros = Macros::default().with('m', quine(&mut names));
    let mut expander = Expander::with_budget(Budget::DEFAULT.with_max_expansions(1000));
    assert_eq!(
        rescan(&mut expander, &macros, "m!(m !)"),
        Err(ExpandError::Budget(Limit::Expansions { max: 1000 }))
    );
}

#[test]
fn reported_quine_terminates_under_the_default_budget() {
    // No configuration at all: the default budget alone bounds the loop.
    let mut names = Interner::new();
    let macros = Macros::default().with('m', quine(&mut names));
    let mut expander = Expander::new();
    let started = Instant::now();
    // Each round writes six tokens, so the token budget runs out first:
    // after 699 050 rounds, 4 194 300 tokens are written and a 699 051st
    // round would pass 2^22.
    assert_eq!(
        rescan(&mut expander, &macros, "m!(m !)"),
        Err(ExpandError::Budget(Limit::Tokens {
            max: Budget::DEFAULT.max_tokens
        }))
    );
    assert_eq!(expansions(&expander), Budget::DEFAULT.max_tokens / 6);
    // Generous even for an unoptimized build on a slow machine; the point is
    // that it ends, not how fast.
    assert!(started.elapsed() < Duration::from_secs(120));
}

#[test]
fn quine_that_adds_a_literal_hits_the_limit_through_hygiene() {
    // m!($a $b $($r)*) => $a $b ($a $b x $($r)*) — the name is captured, but
    // the template writes `x` into the arguments, and `x` carries the depth.
    let mut names = Interner::new();
    let (a, b, r) = (names.intern("a"), names.intern("b"), names.intern("r"));
    let mac = Macro::new(
        names.intern("m"),
        vec![Rule {
            pattern: vec![tt(a), tt(b), star(r)],
            template: vec![
                Template::Var(a),
                Template::Var(b),
                tgroup(
                    '(',
                    vec![
                        Template::Var(a),
                        Template::Var(b),
                        lit('x'),
                        trep(vec![Template::Var(r)]),
                    ],
                ),
            ],
        }],
    )
    .unwrap();
    let macros = Macros::default().with('m', mac);
    let mut expander = Expander::with_limit(12);
    assert_eq!(
        rescan(&mut expander, &macros, "m!(m !)"),
        Err(ExpandError::RecursionLimit { limit: 12 })
    );
    assert_eq!(expander.usage().expansions, 12);
}

#[test]
fn quine_nested_in_a_group_hits_the_limit() {
    // m!($a $b) => { $a $b ($a $b) } — the rebuilt call hides inside braces.
    let mut names = Interner::new();
    let (a, b) = (names.intern("a"), names.intern("b"));
    let mac = Macro::new(
        names.intern("m"),
        vec![Rule {
            pattern: vec![tt(a), tt(b)],
            template: vec![tgroup(
                '{',
                vec![
                    Template::Var(a),
                    Template::Var(b),
                    tgroup('(', vec![Template::Var(a), Template::Var(b)]),
                ],
            )],
        }],
    )
    .unwrap();
    let macros = Macros::default().with('m', mac);
    let mut expander = Expander::with_limit(10);
    let (result, deepest) = run_tracked(&mut expander, &macros, "m!(m !)");
    assert_eq!(result, Err(ExpandError::RecursionLimit { limit: 10 }));
    assert_eq!(deepest, 10);
}

#[test]
fn mutually_recursive_quines_hit_the_limit() {
    // p!($a $b) => q ! ($a $b)  and  q!($a $b) => $a $b ($a $b) — the name of
    // the second call is a literal, the third is rebuilt from captures.
    let mut names = Interner::new();
    let (a, b) = (names.intern("a"), names.intern("b"));
    let p = Macro::new(
        names.intern("p"),
        vec![Rule {
            pattern: vec![tt(a), tt(b)],
            template: vec![
                lit('q'),
                lit('!'),
                tgroup('(', vec![Template::Var(a), Template::Var(b)]),
            ],
        }],
    )
    .unwrap();
    let q = quine(&mut names);
    let macros = Macros::default().with('p', p).with('q', q);
    let mut expander = Expander::with_limit(20);
    // `q!(p !)` writes `p ! (p !)`, which writes `q ! (p !)`, and so on.
    let (result, deepest) = run_tracked(&mut expander, &macros, "q!(p !)");
    assert_eq!(result, Err(ExpandError::RecursionLimit { limit: 20 }));
    assert_eq!(deepest, 20);
}

#[test]
fn fan_out_quine_is_bounded_by_depth_or_budget() {
    // m!($a $b) => $a $b ($a $b) $a $b ($a $b) — two rebuilt calls per round.
    let mut names = Interner::new();
    let (a, b) = (names.intern("a"), names.intern("b"));
    let call = || {
        vec![
            Template::Var(a),
            Template::Var(b),
            tgroup('(', vec![Template::Var(a), Template::Var(b)]),
        ]
    };
    let mac = Macro::new(
        names.intern("m"),
        vec![Rule {
            pattern: vec![tt(a), tt(b)],
            template: [call(), call()].concat(),
        }],
    )
    .unwrap();
    let macros = Macros::default().with('m', mac);

    // A shallow limit is reached first: the leftmost chain is 6 deep.
    let mut expander = Expander::with_limit(6);
    let (result, deepest) = run_tracked(&mut expander, &macros, "m!(m !)");
    assert_eq!(result, Err(ExpandError::RecursionLimit { limit: 6 }));
    assert_eq!(deepest, 6);

    // At the default depth the leftmost chain reaches the limit after 128
    // expansions, long before the 2^128 the whole tree would take.
    let mut expander = Expander::new();
    let (result, deepest) = run_tracked(&mut expander, &macros, "m!(m !)");
    assert_eq!(result, Err(ExpandError::RecursionLimit { limit: 128 }));
    assert_eq!(deepest, 128);
    assert_eq!(expansions(&expander), 128);
}

/// The number of expansions charged to an expander's budget.
fn expansions(expander: &Expander<char>) -> usize {
    expander.usage().expansions
}

#[test]
fn deeper_input_raises_the_depth_but_never_lowers_it() {
    // An invocation runs one level below the deepest of: the driver's depth,
    // its name token, and its input tokens.
    let mut names = Interner::new();
    let x = names.intern("x");
    let id = Macro::new(
        names.intern("id"),
        vec![Rule {
            pattern: vec![star(x)],
            template: vec![lit('k'), trep(vec![Template::Var(x)])],
        }],
    )
    .unwrap();
    let mut expander = Expander::with_limit(3);
    // Three nested expansions: each output's `k` is one level deeper.
    let mut input = Vec::new();
    for _ in 0..3 {
        input = expander.expand(&id, &input, sp(), Context::ROOT).unwrap();
    }
    // The input now holds a level-3 token, so a fourth call is refused even
    // though the name token is in source and the driver claims depth 0.
    assert_eq!(
        expander.expand(&id, &input, sp(), Context::ROOT),
        Err(ExpandError::RecursionLimit { limit: 3 })
    );
    // An explicit depth is honoured when it is the deepest fact.
    assert_eq!(
        expander.expand_at(&id, &[], sp(), Context::ROOT, 3),
        Err(ExpandError::RecursionLimit { limit: 3 })
    );
    assert!(expander.expand_at(&id, &[], sp(), Context::ROOT, 2).is_ok());
}

#[test]
fn expand_is_expand_at_depth_zero() {
    let mut names = Interner::new();
    let x = names.intern("x");
    let mac = Macro::new(
        names.intern("m"),
        vec![Rule {
            pattern: vec![star(x)],
            template: vec![lit('k'), trep(vec![Template::Var(x)])],
        }],
    )
    .unwrap();
    let input = trees("a (b c) d");
    let mut one = Expander::new();
    let mut two = Expander::new();
    let a = one.expand(&mac, &input, sp(), Context::ROOT).unwrap();
    let b = two.expand_at(&mac, &input, sp(), Context::ROOT, 0).unwrap();
    assert_eq!(a, b);
    assert_eq!(one.usage(), two.usage());
}

#[test]
fn ordinary_recursion_is_unaffected() {
    // count!($x $($rest)*) => 1 + count!($($rest)*) still counts.
    let mut names = Interner::new();
    let (x, rest) = (names.intern("x"), names.intern("rest"));
    let count = Macro::new(
        names.intern("count"),
        vec![
            Rule {
                pattern: vec![],
                template: vec![lit('0')],
            },
            Rule {
                pattern: vec![tt(x), star(rest)],
                template: vec![
                    lit('1'),
                    lit('+'),
                    lit('c'),
                    lit('!'),
                    tgroup('(', vec![trep(vec![Template::Var(rest)])]),
                ],
            },
        ],
    )
    .unwrap();
    let macros = Macros::default().with('c', count);
    let out = rescan(&mut Expander::new(), &macros, "c!(a b c d e)").unwrap();
    assert_eq!(render(&out), "1+1+1+1+1+0");
    let (out, deepest) = run_tracked(&mut Expander::new(), &macros, "c!(a b c d e)");
    assert_eq!(render(&out.unwrap()), "1+1+1+1+1+0");
    assert_eq!(deepest, 6);
    // Exactly at the limit is allowed; one below is not.
    let (out, _) = run_tracked(&mut Expander::with_limit(6), &macros, "c!(a b c d e)");
    assert!(out.is_ok());
    let (out, _) = run_tracked(&mut Expander::with_limit(5), &macros, "c!(a b c d e)");
    assert_eq!(out, Err(ExpandError::RecursionLimit { limit: 5 }));
}

// ---------------------------------------------------------------------------
// H08: the budget stops expansion bombs in bounded time and memory
// ---------------------------------------------------------------------------

#[test]
fn doubling_macro_stops_with_a_token_budget_error() {
    let mut names = Interner::new();
    let macros = Macros::default().with('d', dup(&mut names, 'd'));
    let mut expander = Expander::with_budget(Budget::DEFAULT.with_max_tokens(10_000));
    assert_eq!(
        rescan(&mut expander, &macros, "d!(x)"),
        Err(ExpandError::Budget(Limit::Tokens { max: 10_000 }))
    );
}

#[test]
fn doubling_macro_with_a_captured_name_stops_too() {
    // dup!($n $b $($t)*) => $n $b ($n $b $($t)* $($t)*) — no literal name.
    let mut names = Interner::new();
    let (n, b, t) = (names.intern("n"), names.intern("b"), names.intern("t"));
    let mac = Macro::new(
        names.intern("dup"),
        vec![Rule {
            pattern: vec![tt(n), tt(b), star(t)],
            template: vec![
                Template::Var(n),
                Template::Var(b),
                tgroup(
                    '(',
                    vec![
                        Template::Var(n),
                        Template::Var(b),
                        trep(vec![Template::Var(t)]),
                        trep(vec![Template::Var(t)]),
                    ],
                ),
            ],
        }],
    )
    .unwrap();
    let macros = Macros::default().with('d', mac);
    let mut expander = Expander::with_budget(Budget::DEFAULT.with_max_tokens(50_000));
    assert_eq!(
        rescan(&mut expander, &macros, "d!(d ! x)"),
        Err(ExpandError::Budget(Limit::Tokens { max: 50_000 }))
    );
}

#[test]
fn doubling_macro_terminates_under_the_default_budget() {
    let mut names = Interner::new();
    let macros = Macros::default().with('d', dup(&mut names, 'd'));
    let mut expander = Expander::new();
    let started = Instant::now();
    assert_eq!(
        rescan(&mut expander, &macros, "d!(x)"),
        Err(ExpandError::Budget(Limit::Tokens {
            max: Budget::DEFAULT.max_tokens
        }))
    );
    // About twenty rounds of doubling fit in 2^22 tokens.
    let rounds = expansions(&expander);
    assert!((15..=25).contains(&rounds), "{rounds} rounds");
    assert!(started.elapsed() < Duration::from_secs(120));
}

#[test]
fn fan_out_bomb_stops_with_an_expansion_budget_error() {
    // b!() => ;  b!($x $($r)*) => b!($($r)*) b!($($r)*)
    // Every call terminates, nesting stays below the input length, and the
    // tiny outputs never threaten the token budget — but there are 2^(n+1)
    // calls in all.
    let mut names = Interner::new();
    let (x, r) = (names.intern("x"), names.intern("r"));
    let call = || {
        vec![
            lit('b'),
            lit('!'),
            tgroup('(', vec![trep(vec![Template::Var(r)])]),
        ]
    };
    let bomb = Macro::new(
        names.intern("bomb"),
        vec![
            Rule {
                pattern: vec![],
                template: vec![],
            },
            Rule {
                pattern: vec![tt(x), star(r)],
                template: [call(), call()].concat(),
            },
        ],
    )
    .unwrap();
    let macros = Macros::default().with('b', bomb);
    let src = format!("b!({})", "x ".repeat(40));
    let mut expander = Expander::with_budget(Budget::DEFAULT.with_max_expansions(10_000));
    let (result, deepest) = run_tracked(&mut expander, &macros, &src);
    assert_eq!(
        result,
        Err(ExpandError::Budget(Limit::Expansions { max: 10_000 }))
    );
    assert!(deepest <= 41, "stopped by the budget, not the depth limit");
    assert_eq!(expansions(&expander), 10_000);

    // The same holds through hygiene alone.
    let mut expander = Expander::with_budget(Budget::DEFAULT.with_max_expansions(5_000));
    assert_eq!(
        rescan(&mut expander, &macros, &src),
        Err(ExpandError::Budget(Limit::Expansions { max: 5_000 }))
    );
}

#[test]
fn token_budget_is_exact_at_the_boundary() {
    // id!($($t)*) => $($t)* over `a (b c) [d]`: 4 tokens + 2 groups = 8.
    let mut names = Interner::new();
    let t = names.intern("t");
    let id = Macro::new(
        names.intern("id"),
        vec![Rule {
            pattern: vec![star(t)],
            template: vec![trep(vec![Template::Var(t)])],
        }],
    )
    .unwrap();
    let input = trees("a (b c) [d]");
    assert_eq!(token_count(&input), 8);

    let mut exact = Expander::with_budget(Budget::DEFAULT.with_max_tokens(8));
    assert_eq!(
        exact.expand(&id, &input, sp(), Context::ROOT).unwrap(),
        input
    );
    // The budget is spent: even one more token is refused.
    assert_eq!(
        exact.expand(&id, &trees("z"), sp(), Context::ROOT),
        Err(ExpandError::Budget(Limit::Tokens { max: 8 }))
    );
    // ...but an expansion that writes nothing still fits.
    assert_eq!(exact.expand(&id, &[], sp(), Context::ROOT), Ok(vec![]));

    let mut short = Expander::with_budget(Budget::DEFAULT.with_max_tokens(7));
    assert_eq!(
        short.expand(&id, &input, sp(), Context::ROOT),
        Err(ExpandError::Budget(Limit::Tokens { max: 7 }))
    );
}

#[test]
fn every_kind_of_written_token_is_charged() {
    // m!($($x),*) => k [ ] $($x);*
    let mut names = Interner::new();
    let x = names.intern("x");
    let mac = Macro::new(
        names.intern("m"),
        vec![Rule {
            pattern: vec![Pattern::Repeat {
                body: vec![tt(x)],
                separator: Some(','),
                kleene: Kleene::ZeroOrMore,
            }],
            template: vec![
                lit('k'),
                tgroup('[', vec![]),
                Template::Repeat {
                    body: vec![Template::Var(x)],
                    separator: Some(Token::new(';', sp())),
                },
            ],
        }],
    )
    .unwrap();
    // Output `k [] (a) ; b ; c`: literal 1 + group 2 + captures 3+1+1 +
    // separators 2 = 10.
    let input = trees("(a), b, c");
    let mut fits = Expander::with_budget(Budget::DEFAULT.with_max_tokens(10));
    let out = fits.expand(&mac, &input, sp(), Context::ROOT).unwrap();
    assert_eq!(render(&out), "k[](a);b;c");
    assert_eq!(token_count(&out), 10);
    let mut tight = Expander::with_budget(Budget::DEFAULT.with_max_tokens(9));
    assert_eq!(
        tight.expand(&mac, &input, sp(), Context::ROOT),
        Err(ExpandError::Budget(Limit::Tokens { max: 9 }))
    );
}

#[test]
fn oversized_capture_is_refused_and_leaves_no_trace() {
    let mut names = Interner::new();
    let t = names.intern("t");
    // big!($t:tt) => k $t — the literal mints a context before the capture
    // is refused; the failure must roll it back.
    let big = Macro::new(
        names.intern("big"),
        vec![Rule {
            pattern: vec![tt(t)],
            template: vec![lit('k'), Template::Var(t)],
        }],
    )
    .unwrap();
    let huge = vec![Tree::Group {
        open: Token::new('(', sp()),
        close: Token::new(')', sp()),
        trees: (0..100_000).map(|_| Tree::token('a', sp())).collect(),
    }];
    let mut expander = Expander::with_budget(Budget::DEFAULT.with_max_tokens(1000));
    assert_eq!(
        expander.expand(&big, &huge, sp(), Context::ROOT),
        Err(ExpandError::Budget(Limit::Tokens { max: 1000 }))
    );
    assert_eq!(expander.usage(), Usage::NONE);
    // The whole budget is still available afterwards.
    let small = trees("(a b)");
    let out = expander.expand(&big, &small, sp(), Context::ROOT).unwrap();
    assert_eq!(render(&out), "k(ab)");
    assert_eq!(contexts(&out)[0].1.as_u32(), 1, "first context reused");
}

#[test]
fn zero_budgets_reject_every_expansion() {
    let mut names = Interner::new();
    let empty = Macro::<char>::new(
        names.intern("empty"),
        vec![Rule {
            pattern: vec![],
            template: vec![],
        }],
    )
    .unwrap();
    let mut none = Expander::with_budget(Budget::DEFAULT.with_max_expansions(0));
    assert_eq!(
        none.expand(&empty, &[], sp(), Context::ROOT),
        Err(ExpandError::Budget(Limit::Expansions { max: 0 }))
    );
    // A zero token budget still allows an expansion that writes nothing.
    let mut silent = Expander::with_budget(Budget::DEFAULT.with_max_tokens(0));
    assert_eq!(silent.expand(&empty, &[], sp(), Context::ROOT), Ok(vec![]));
    // Depth is checked first and keeps its own error.
    let mut neither =
        Expander::with_budget(Budget::DEFAULT.with_max_depth(0).with_max_expansions(0));
    assert_eq!(
        neither.expand(&empty, &[], sp(), Context::ROOT),
        Err(ExpandError::RecursionLimit { limit: 0 })
    );
}

#[test]
fn failed_expansions_are_not_charged() {
    let mut names = Interner::new();
    let only_a = Macro::new(
        names.intern("only_a"),
        vec![Rule {
            pattern: vec![Pattern::Token('a')],
            template: vec![lit('k')],
        }],
    )
    .unwrap();
    let mut expander = Expander::with_budget(Budget::DEFAULT.with_max_expansions(1));
    for _ in 0..10 {
        assert!(matches!(
            expander.expand(&only_a, &trees("b"), sp(), Context::ROOT),
            Err(ExpandError::NoMatch { .. })
        ));
    }
    assert!(
        expander
            .expand(&only_a, &trees("a"), sp(), Context::ROOT)
            .is_ok()
    );
    assert_eq!(
        expander.expand(&only_a, &trees("a"), sp(), Context::ROOT),
        Err(ExpandError::Budget(Limit::Expansions { max: 1 }))
    );
}

#[test]
fn set_budget_raises_and_lowers_the_lifetime_limits() {
    let mut names = Interner::new();
    let one = Macro::<char>::new(
        names.intern("one"),
        vec![Rule {
            pattern: vec![],
            template: vec![lit('k')],
        }],
    )
    .unwrap();
    let mut expander = Expander::with_budget(Budget::DEFAULT.with_max_tokens(2));
    assert!(expander.expand(&one, &[], sp(), Context::ROOT).is_ok());
    assert!(expander.expand(&one, &[], sp(), Context::ROOT).is_ok());
    assert_eq!(
        expander.expand(&one, &[], sp(), Context::ROOT),
        Err(ExpandError::Budget(Limit::Tokens { max: 2 }))
    );
    expander.set_budget(expander.budget().with_max_tokens(3));
    assert!(expander.expand(&one, &[], sp(), Context::ROOT).is_ok());
    expander.set_budget(Budget::DEFAULT.with_max_expansions(1));
    assert_eq!(
        expander.expand(&one, &[], sp(), Context::ROOT),
        Err(ExpandError::Budget(Limit::Expansions { max: 1 }))
    );
    // `with_limit` is the default budget with a different depth.
    assert_eq!(
        Expander::<char>::with_limit(7).budget(),
        Budget::DEFAULT.with_max_depth(7)
    );
}

// ---------------------------------------------------------------------------
// M70: repetition separators are marked from their definition context
// ---------------------------------------------------------------------------

/// Expands `make!()` once and returns the (non-root) context its template
/// token received — a definition context for a macro "defined by a macro".
fn minted_context(expander: &mut Expander<char>, names: &mut Interner) -> Context {
    let make = Macro::new(
        names.intern("make"),
        vec![Rule {
            pattern: vec![],
            template: vec![lit('k')],
        }],
    )
    .unwrap();
    let out = expander.expand(&make, &[], sp(), Context::ROOT).unwrap();
    contexts(&out)[0].1
}

#[test]
fn separator_shares_the_context_of_literals_defined_alongside_it() {
    let mut names = Interner::new();
    let mut expander = Expander::new();
    let def = minted_context(&mut expander, &mut names);
    assert!(!def.is_root());

    // join!($($x),*) => t $($x);*   with `t` and `;` both defined in `def`
    let x = names.intern("x");
    let join = Macro::new(
        names.intern("join"),
        vec![Rule {
            pattern: vec![Pattern::Repeat {
                body: vec![tt(x)],
                separator: Some(','),
                kleene: Kleene::ZeroOrMore,
            }],
            template: vec![
                Template::Token {
                    token: Token::new('t', sp()),
                    ctx: def,
                },
                Template::RepeatSeparated {
                    body: vec![Template::Var(x)],
                    separator: Token::new(';', sp()),
                    ctx: def,
                },
            ],
        }],
    )
    .unwrap();
    let out = expander
        .expand(&join, &trees("a, b, c"), sp(), Context::ROOT)
        .unwrap();
    assert_eq!(render(&out), "ta;b;c");
    let marks = contexts(&out);
    let t_ctx = marks[0].1;
    let separators: Vec<Context> = marks
        .iter()
        .filter(|(kind, _)| *kind == ';')
        .map(|(_, ctx)| *ctx)
        .collect();
    assert_eq!(
        separators,
        vec![t_ctx, t_ctx],
        "one minted context per definition context"
    );
    let origin = expander.origin(t_ctx).unwrap();
    assert_eq!(origin.parent, def);
    // Captures are untouched.
    assert!(
        marks
            .iter()
            .filter(|(k, _)| "abc".contains(*k))
            .all(|(_, c)| c.is_root())
    );
}

#[test]
fn plain_repeat_separator_keeps_its_root_definition_context() {
    // The 1.0 behaviour of `Template::Repeat` is unchanged: its separator is
    // defined in the root context, like `Template::token`.
    let mut names = Interner::new();
    let x = names.intern("x");
    let mac = Macro::new(
        names.intern("m"),
        vec![Rule {
            pattern: vec![star(x)],
            template: vec![
                lit('t'),
                Template::Repeat {
                    body: vec![Template::Var(x)],
                    separator: Some(Token::new(';', sp())),
                },
            ],
        }],
    )
    .unwrap();
    let mut expander = Expander::new();
    let out = expander
        .expand(&mac, &trees("a b"), sp(), Context::ROOT)
        .unwrap();
    let marks = contexts(&out);
    assert_eq!(marks[0].1, marks[2].1, "`t` and `;` share a context");
    assert_eq!(expander.origin(marks[2].1).unwrap().parent, Context::ROOT);
}

#[test]
fn repeat_separated_in_root_matches_plain_repeat() {
    let mut names = Interner::new();
    let x = names.intern("x");
    let build = |template| {
        Macro::new(
            Symbol::from_u32(99).unwrap(),
            vec![Rule {
                pattern: vec![star(x)],
                template: vec![lit('t'), template],
            }],
        )
        .unwrap()
    };
    let plain = build(Template::Repeat {
        body: vec![Template::Var(x)],
        separator: Some(Token::new(',', sp())),
    });
    let explicit = build(Template::RepeatSeparated {
        body: vec![Template::Var(x)],
        separator: Token::new(',', sp()),
        ctx: Context::ROOT,
    });
    let input = trees("a (b) c");
    let a = Expander::new()
        .expand(&plain, &input, sp(), Context::ROOT)
        .unwrap();
    let b = Expander::new()
        .expand(&explicit, &input, sp(), Context::ROOT)
        .unwrap();
    assert_eq!(a, b);
}

#[test]
fn separator_with_a_distinct_context_gets_its_own_mark() {
    // Separator captured from a different definition than the literals: two
    // distinct definition contexts mint two distinct contexts.
    let mut names = Interner::new();
    let mut expander = Expander::new();
    let def = minted_context(&mut expander, &mut names);
    let x = names.intern("x");
    let mac = Macro::new(
        names.intern("m"),
        vec![Rule {
            pattern: vec![star(x)],
            template: vec![
                lit('t'),
                Template::RepeatSeparated {
                    body: vec![Template::Var(x)],
                    separator: Token::new(';', sp()),
                    ctx: def,
                },
            ],
        }],
    )
    .unwrap();
    let out = expander
        .expand(&mac, &trees("a b"), sp(), Context::ROOT)
        .unwrap();
    let marks = contexts(&out);
    assert_ne!(marks[0].1, marks[2].1);
    assert_eq!(expander.origin(marks[0].1).unwrap().parent, Context::ROOT);
    assert_eq!(expander.origin(marks[2].1).unwrap().parent, def);
}

#[test]
fn repeat_separated_follows_the_definition_checks() {
    let mut names = Interner::new();
    // A repetition with no repeating metavariable is still rejected.
    let err = Macro::<char>::new(
        names.intern("m"),
        vec![Rule {
            pattern: vec![],
            template: vec![Template::RepeatSeparated {
                body: vec![lit('k')],
                separator: Token::new(',', sp()),
                ctx: Context::ROOT,
            }],
        }],
    )
    .unwrap_err();
    assert_eq!(err, macro_lang::MacroError::NoRepeatingVariable { rule: 0 });
}

// ---------------------------------------------------------------------------
// properties
// ---------------------------------------------------------------------------

/// A template built only from the two captured names and groups — no literal
/// tokens, so hygiene alone could never see how deep a rebuilt call is.
#[derive(Clone, Debug)]
enum Shape {
    A,
    B,
    Group(Vec<Shape>),
}

fn shape() -> impl Strategy<Value = Shape> {
    prop_oneof![Just(Shape::A), Just(Shape::B)].prop_recursive(3, 16, 4, |inner| {
        prop::collection::vec(inner, 0..4).prop_map(Shape::Group)
    })
}

fn lower(shapes: &[Shape], a: Symbol, b: Symbol) -> Vec<Template<char>> {
    shapes
        .iter()
        .map(|shape| match shape {
            Shape::A => Template::Var(a),
            Shape::B => Template::Var(b),
            Shape::Group(body) => tgroup('(', lower(body, a, b)),
        })
        .collect()
}

/// A random tree of up to `depth` levels, for the token-count property.
fn random_trees() -> impl Strategy<Value = Vec<Tree<char>>> {
    let leaf = prop_oneof![Just('a'), Just('b'), Just('!')].prop_map(|c| Tree::token(c, sp()));
    let tree = leaf.prop_recursive(4, 32, 4, |inner| {
        prop::collection::vec(inner, 0..4).prop_map(|trees| Tree::Group {
            open: Token::new('(', sp()),
            close: Token::new(')', sp()),
            trees,
        })
    });
    prop::collection::vec(tree, 0..8)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// For any template made only of captures and groups — including every
    /// shape that rebuilds its own invocation — a tracking driver never runs
    /// an expansion deeper than the limit, and always terminates.
    #[test]
    fn tracked_depth_never_exceeds_the_limit(template in prop::collection::vec(shape(), 0..6), limit in 0u32..12) {
        let mut names = Interner::new();
        let (a, b) = (names.intern("a"), names.intern("b"));
        let mac = Macro::new(names.intern("m"), vec![Rule {
            pattern: vec![tt(a), tt(b)],
            template: lower(&template, a, b),
        }]).unwrap();
        let macros = Macros::default().with('m', mac);
        let budget = Budget::DEFAULT.with_max_depth(limit).with_max_expansions(2000).with_max_tokens(200_000);
        let mut expander = Expander::with_budget(budget);
        let (result, deepest) = run_tracked(&mut expander, &macros, "m!(m !)");
        prop_assert!(deepest <= limit);
        match result {
            // A rebuilt call may not fit the two-tree pattern: `NoMatch` is
            // a legitimate end too.
            Ok(_)
            | Err(
                ExpandError::RecursionLimit { .. }
                | ExpandError::Budget(_)
                | ExpandError::NoMatch { .. },
            ) => {}
            Err(other) => prop_assert!(false, "unexpected error {other:?}"),
        }
    }

    /// The token charge is exactly the size of the output: a budget equal to
    /// it succeeds, one token less fails, and a failure changes nothing.
    #[test]
    fn token_charge_equals_output_size(input in random_trees()) {
        let mut names = Interner::new();
        let t = names.intern("t");
        // m!($($t)*) => [ $($t)* ] $($t)*
        let mac = Macro::new(names.intern("m"), vec![Rule {
            pattern: vec![star(t)],
            template: vec![tgroup('[', vec![trep(vec![Template::Var(t)])]), trep(vec![Template::Var(t)])],
        }]).unwrap();
        let size = 2 + 2 * token_count(&input);
        let mut exact = Expander::with_budget(Budget::DEFAULT.with_max_tokens(size));
        let out = exact.expand(&mac, &input, sp(), Context::ROOT).unwrap();
        prop_assert_eq!(token_count(&out), size);
        let mut short = Expander::with_budget(Budget::DEFAULT.with_max_tokens(size - 1));
        prop_assert_eq!(
            short.expand(&mac, &input, sp(), Context::ROOT),
            Err(ExpandError::Budget(Limit::Tokens { max: size - 1 }))
        );
        prop_assert_eq!(short.usage(), Usage::NONE);
        prop_assert_eq!(exact.usage().tokens, size);
    }

    /// Cumulative accounting: a run of expansions under a token budget
    /// succeeds exactly while the running total of output sizes fits.
    #[test]
    fn cumulative_tokens_match_a_running_total(
        inputs in prop::collection::vec(random_trees(), 1..8),
        max in 0usize..120,
    ) {
        let mut names = Interner::new();
        let t = names.intern("t");
        let id = Macro::new(names.intern("id"), vec![Rule {
            pattern: vec![star(t)],
            template: vec![trep(vec![Template::Var(t)])],
        }]).unwrap();
        let mut expander = Expander::with_budget(Budget::DEFAULT.with_max_tokens(max));
        let mut total = 0;
        for input in &inputs {
            let size = token_count(input);
            let result = expander.expand(&id, input, sp(), Context::ROOT);
            if total + size <= max {
                prop_assert_eq!(result, Ok(input.clone()));
                total += size;
            } else {
                prop_assert_eq!(result, Err(ExpandError::Budget(Limit::Tokens { max })));
            }
        }
    }
}

#[test]
fn deeply_nested_capture_is_charged_in_full() {
    // A 2 000-deep group is one captured tree of 4 001 tokens: charged as a
    // whole before it is copied, without recursion.
    let depth = 2_000;
    let src = format!("{}x{}", "(".repeat(depth), ")".repeat(depth));
    let input = trees(&src);
    let mut names = Interner::new();
    let t = names.intern("t");
    let id = Macro::new(
        names.intern("id"),
        vec![Rule {
            pattern: vec![tt(t)],
            template: vec![Template::Var(t)],
        }],
    )
    .unwrap();
    let mut short = Expander::with_budget(Budget::DEFAULT.with_max_tokens(2 * depth));
    assert_eq!(
        short.expand(&id, &input, sp(), Context::ROOT),
        Err(ExpandError::Budget(Limit::Tokens { max: 2 * depth }))
    );
    let mut exact = Expander::with_budget(Budget::DEFAULT.with_max_tokens(2 * depth + 1));
    assert_eq!(
        exact.expand(&id, &input, sp(), Context::ROOT).unwrap(),
        input
    );
}

// ---------------------------------------------------------------------------
// usage and reset: per-unit budgets for long-lived hosts
// ---------------------------------------------------------------------------

#[test]
fn usage_counts_successful_expansions_and_their_tokens() {
    let mut names = Interner::new();
    let t = names.intern("t");
    let id = Macro::new(
        names.intern("id"),
        vec![Rule {
            pattern: vec![star(t)],
            template: vec![trep(vec![Template::Var(t)])],
        }],
    )
    .unwrap();
    let only_a = Macro::new(
        names.intern("only_a"),
        vec![Rule {
            pattern: vec![Pattern::Token('a')],
            template: vec![lit('k')],
        }],
    )
    .unwrap();
    let mut expander = Expander::new();
    assert_eq!(expander.usage(), Usage::NONE);
    let _ = expander
        .expand(&id, &trees("a (b c)"), sp(), Context::ROOT)
        .unwrap();
    let _ = expander.expand(&id, &[], sp(), Context::ROOT).unwrap();
    // Failures are not charged.
    assert!(
        expander
            .expand(&only_a, &trees("b"), sp(), Context::ROOT)
            .is_err()
    );
    let usage = expander.usage();
    assert_eq!((usage.expansions, usage.tokens), (2, 5));
}

#[test]
fn reset_usage_restores_the_full_budget_per_unit_of_work() {
    // A host that resets per unit of work: each unit may use the whole
    // budget, however many units came before.
    let mut names = Interner::new();
    let three = Macro::<char>::new(
        names.intern("three"),
        vec![Rule {
            pattern: vec![],
            template: vec![lit('a'), lit('b'), lit('c')],
        }],
    )
    .unwrap();
    let mut expander = Expander::with_budget(Budget::DEFAULT.with_max_tokens(6));
    for _unit in 0..100 {
        expander.reset_usage();
        assert_eq!(expander.usage(), Usage::NONE);
        assert!(expander.expand(&three, &[], sp(), Context::ROOT).is_ok());
        assert!(expander.expand(&three, &[], sp(), Context::ROOT).is_ok());
        assert_eq!(
            expander.expand(&three, &[], sp(), Context::ROOT),
            Err(ExpandError::Budget(Limit::Tokens { max: 6 }))
        );
    }
    // Without a reset the budget stays spent.
    assert_eq!(
        expander.expand(&three, &[], sp(), Context::ROOT),
        Err(ExpandError::Budget(Limit::Tokens { max: 6 }))
    );
}

#[test]
fn reset_usage_leaves_hygiene_and_budget_untouched() {
    let mut names = Interner::new();
    let make = Macro::<char>::new(
        names.intern("make"),
        vec![Rule {
            pattern: vec![],
            template: vec![lit('k')],
        }],
    )
    .unwrap();
    let budget = Budget::DEFAULT.with_max_depth(9).with_max_expansions(50);
    let mut expander = Expander::with_budget(budget);
    let before = expander
        .expand(&make, &[], Span::new(3, 4), Context::ROOT)
        .unwrap();
    let ctx = contexts(&before)[0].1;
    let origin = expander.origin(ctx).unwrap();
    expander.reset_usage();
    assert_eq!(expander.budget(), budget);
    assert_eq!(expander.origin(ctx), Some(origin));
    // Contexts keep counting densely: the next expansion mints a new one.
    let after = expander.expand(&make, &[], sp(), Context::ROOT).unwrap();
    let next = contexts(&after)[0].1;
    assert_ne!(next, ctx);
    assert_eq!(next.as_u32(), ctx.as_u32() + 1);
    // Depth is hygiene, not usage: a reset does not reset nesting.
    let mut deep = Expander::with_limit(2);
    let first = deep.expand(&make, &[], sp(), Context::ROOT).unwrap();
    let call = contexts(&first)[0].1;
    let second = deep.expand(&make, &[], sp(), call).unwrap();
    deep.reset_usage();
    assert_eq!(
        deep.expand(&make, &[], sp(), contexts(&second)[0].1),
        Err(ExpandError::RecursionLimit { limit: 2 })
    );
}

#[test]
fn reset_lets_a_host_recover_after_a_bomb() {
    // One hostile document must not poison the next one.
    let mut names = Interner::new();
    let macros = Macros::default().with('d', dup(&mut names, 'd'));
    let mut expander = Expander::with_budget(Budget::DEFAULT.with_max_tokens(4096));
    assert_eq!(
        rescan(&mut expander, &macros, "d!(x)"),
        Err(ExpandError::Budget(Limit::Tokens { max: 4096 }))
    );
    expander.reset_usage();
    let three = Macro::<char>::new(
        names.intern("three"),
        vec![Rule {
            pattern: vec![],
            template: vec![lit('a'), lit('b'), lit('c')],
        }],
    )
    .unwrap();
    let out = expander.expand(&three, &[], sp(), Context::ROOT).unwrap();
    assert_eq!(render(&out), "abc");
    assert_eq!(expander.usage().tokens, 3);
}
