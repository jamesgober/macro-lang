//! Criterion benchmarks for the expansion hot path.
//!
//! Each benchmark reuses one `Expander`, as a real expansion driver does, so
//! the numbers reflect steady state: pooled buffers warm, only the output trees
//! allocated.
//!
//! - `list/N` — `$($e:tt),*  =>  [ $($e);* ]` over `N` elements: the common
//!   separated-repetition macro, scaling with input length.
//! - `nested/N` — `$( ( $($x:tt)* ) )*` over `N` groups of four tokens:
//!   group descent and two-level repetition bookkeeping.
//! - `dispatch/rules=16` — sixteen rules distinguished by a leading literal,
//!   invoking the last: the cost of rules that fail on their first token.
//! - `hygiene/literals=64` — a template that writes sixty-four literal tokens:
//!   context minting and token output.
//! - `identity/depth=64` — `$($t:tt)*` over a 64-deep nested group: flattening
//!   and rebuilding a deep capture.
//! - `budget/dup/N` — a doubling macro (`dup!($($t)*) => dup!($($t)* $($t)*)`)
//!   driven until a token budget of `N` stops it: the cost of an expansion
//!   bomb is linear in the budget, not exponential in the depth.
//! - `budget/quine/N` — `m!($a $b) => $a $b ($a $b)` re-invoked on its own
//!   output until an expansion budget of `N` stops it: linear in the budget.
//! - `budget/limit=128` — the same macro under `expand_at` with a tracked
//!   depth, stopped by the default recursion limit.
//!
//! The steady-state benchmarks run millions of expansions on one expander, far
//! more than a compilation performs, so they lift the cumulative expansion and
//! token budgets (see `steady`).

use std::hint::black_box;

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use intern_lang::{Interner, Symbol};
use macro_lang::{
    Budget, Context, ExpandError, Expander, Fragment, Kleene, Macro, Pattern, Rule, Template, Tree,
};
use token_lang::{Span, Token};

fn sp() -> Span {
    Span::new(0, 1)
}

fn tok(kind: char) -> Token<char> {
    Token::new(kind, sp())
}

fn tt(name: Symbol) -> Pattern<char> {
    Pattern::Bind {
        name,
        fragment: Fragment::Tree,
    }
}

fn star(body: Vec<Pattern<char>>, separator: Option<char>) -> Pattern<char> {
    Pattern::Repeat {
        body,
        separator,
        kleene: Kleene::ZeroOrMore,
    }
}

/// An expander for steady-state measurement: the default depth limit, but no
/// lifetime cap on expansions or tokens, because a benchmark loop performs
/// millions of expansions on one expander.
fn steady() -> Expander<char> {
    Expander::with_budget(
        Budget::DEFAULT
            .with_max_expansions(usize::MAX)
            .with_max_tokens(usize::MAX),
    )
}

fn call(expander: &mut Expander<char>, mac: &Macro<char>, input: &[Tree<char>]) -> usize {
    expander
        .expand(mac, input, sp(), Context::ROOT)
        .map_or(0, |out| out.len())
}

fn bench_list(c: &mut Criterion) {
    let mut names = Interner::new();
    let e = names.intern("e");
    let mac = Macro::new(
        names.intern("list"),
        vec![Rule {
            pattern: vec![star(vec![tt(e)], Some(','))],
            template: vec![Template::Group {
                open: tok('['),
                close: tok(']'),
                body: vec![Template::Repeat {
                    body: vec![Template::Var(e)],
                    separator: Some(tok(';')),
                }],
            }],
        }],
    )
    .unwrap_or_else(|err| panic!("{err}"));

    let mut group = c.benchmark_group("list");
    for n in [8usize, 64, 512] {
        let mut input = Vec::with_capacity(n * 2);
        for i in 0..n {
            if i > 0 {
                input.push(Tree::token(',', sp()));
            }
            input.push(Tree::token('a', sp()));
        }
        let mut expander = steady();
        group.throughput(Throughput::Elements(n as u64));
        group.bench_with_input(BenchmarkId::from_parameter(n), &input, |b, input| {
            b.iter(|| call(&mut expander, &mac, black_box(input)));
        });
    }
    group.finish();
}

fn bench_nested(c: &mut Criterion) {
    let mut names = Interner::new();
    let x = names.intern("x");
    let mac = Macro::new(
        names.intern("nested"),
        vec![Rule {
            pattern: vec![star(
                vec![Pattern::Group {
                    open: '(',
                    close: ')',
                    body: vec![star(vec![tt(x)], None)],
                }],
                None,
            )],
            template: vec![Template::Repeat {
                body: vec![Template::Group {
                    open: tok('['),
                    close: tok(']'),
                    body: vec![Template::Repeat {
                        body: vec![Template::Var(x)],
                        separator: None,
                    }],
                }],
                separator: None,
            }],
        }],
    )
    .unwrap_or_else(|err| panic!("{err}"));

    let mut group = c.benchmark_group("nested");
    for n in [8usize, 64] {
        let input: Vec<Tree<char>> = (0..n)
            .map(|_| Tree::Group {
                open: tok('('),
                close: tok(')'),
                trees: "abcd".chars().map(|k| Tree::token(k, sp())).collect(),
            })
            .collect();
        let mut expander = steady();
        group.throughput(Throughput::Elements(n as u64));
        group.bench_with_input(BenchmarkId::from_parameter(n), &input, |b, input| {
            b.iter(|| call(&mut expander, &mac, black_box(input)));
        });
    }
    group.finish();
}

fn bench_dispatch(c: &mut Criterion) {
    let mut names = Interner::new();
    let x = names.intern("x");
    let rules = ('a'..='p')
        .map(|lead| Rule {
            pattern: vec![Pattern::Token(lead), tt(x)],
            template: vec![Template::Var(x)],
        })
        .collect();
    let mac = Macro::new(names.intern("dispatch"), rules).unwrap_or_else(|err| panic!("{err}"));
    let input = vec![Tree::token('p', sp()), Tree::token('z', sp())];
    let mut expander = steady();
    c.bench_function("dispatch/rules=16", |b| {
        b.iter(|| call(&mut expander, &mac, black_box(&input)));
    });
}

fn bench_hygiene(c: &mut Criterion) {
    let mut names = Interner::new();
    let template = (0..64)
        .map(|i| Template::token(if i % 2 == 0 { 't' } else { '=' }, sp()))
        .collect();
    let mac = Macro::new(
        names.intern("literals"),
        vec![Rule {
            pattern: vec![],
            template,
        }],
    )
    .unwrap_or_else(|err| panic!("{err}"));
    let mut expander = steady();
    c.bench_function("hygiene/literals=64", |b| {
        b.iter(|| call(&mut expander, &mac, black_box(&[])));
    });
}

fn bench_identity(c: &mut Criterion) {
    let mut names = Interner::new();
    let t = names.intern("t");
    let mac = Macro::new(
        names.intern("identity"),
        vec![Rule {
            pattern: vec![star(vec![tt(t)], None)],
            template: vec![Template::Repeat {
                body: vec![Template::Var(t)],
                separator: None,
            }],
        }],
    )
    .unwrap_or_else(|err| panic!("{err}"));
    let mut deep = Tree::token('x', sp());
    for _ in 0..64 {
        deep = Tree::Group {
            open: tok('('),
            close: tok(')'),
            trees: vec![deep],
        };
    }
    let input = vec![deep];
    let mut expander = steady();
    c.bench_function("identity/depth=64", |b| {
        b.iter(|| call(&mut expander, &mac, black_box(&input)));
    });
}

/// Re-invokes `mac` on the trees inside the group its output ends with —
/// what a rescanning driver does — until expansion fails, and returns the
/// error. The call context is the first output token's, so hygiene sees
/// whatever depth the macro's own literals carry.
fn drive(expander: &mut Expander<char>, mac: &Macro<char>, start: Vec<Tree<char>>) -> ExpandError {
    let mut args = start;
    let mut ctx = Context::ROOT;
    loop {
        match expander.expand(mac, &args, sp(), ctx) {
            Ok(mut out) => {
                if let Some(Tree::Token { ctx: name, .. }) = out.first() {
                    ctx = *name;
                }
                match out.pop() {
                    Some(Tree::Group { trees, .. }) => args = trees,
                    _ => return ExpandError::RecursionLimit { limit: 0 },
                }
            }
            Err(err) => return err,
        }
    }
}

fn bench_budget(c: &mut Criterion) {
    let mut names = Interner::new();
    let (t, a, b) = (names.intern("t"), names.intern("a"), names.intern("b"));
    let group_of = |body| Template::Group {
        open: tok('('),
        close: tok(')'),
        body,
    };
    let repeat_t = || Template::Repeat {
        body: vec![Template::Var(t)],
        separator: None,
    };
    let dup = Macro::new(
        names.intern("dup"),
        vec![Rule {
            pattern: vec![star(vec![tt(t)], None)],
            template: vec![
                Template::token('d', sp()),
                Template::token('!', sp()),
                group_of(vec![repeat_t(), repeat_t()]),
            ],
        }],
    )
    .unwrap_or_else(|err| panic!("{err}"));
    let quine = Macro::new(
        names.intern("quine"),
        vec![Rule {
            pattern: vec![tt(a), tt(b)],
            template: vec![
                Template::Var(a),
                Template::Var(b),
                group_of(vec![Template::Var(a), Template::Var(b)]),
            ],
        }],
    )
    .unwrap_or_else(|err| panic!("{err}"));
    let quine_args = || vec![Tree::token('m', sp()), Tree::token('!', sp())];

    let mut group = c.benchmark_group("budget");
    for max in [1usize << 12, 1 << 14, 1 << 16] {
        group.throughput(Throughput::Elements(max as u64));
        group.bench_with_input(BenchmarkId::new("dup", max), &max, |bench, &max| {
            bench.iter(|| {
                let mut expander = Expander::with_budget(Budget::DEFAULT.with_max_tokens(max));
                drive(&mut expander, &dup, vec![Tree::token('x', sp())])
            });
        });
    }
    for max in [1usize << 10, 1 << 12, 1 << 14] {
        group.throughput(Throughput::Elements(max as u64));
        group.bench_with_input(BenchmarkId::new("quine", max), &max, |bench, &max| {
            bench.iter(|| {
                let mut expander = Expander::with_budget(Budget::DEFAULT.with_max_expansions(max));
                drive(&mut expander, &quine, quine_args())
            });
        });
    }
    group.throughput(Throughput::Elements(128));
    group.bench_function("limit=128", |bench| {
        bench.iter(|| {
            let mut expander: Expander<char> = Expander::new();
            let mut args = quine_args();
            let mut depth = 0;
            loop {
                match expander.expand_at(&quine, &args, sp(), Context::ROOT, depth) {
                    Ok(mut out) => match out.pop() {
                        Some(Tree::Group { trees, .. }) => {
                            args = trees;
                            depth += 1;
                        }
                        _ => break None,
                    },
                    Err(err) => break Some(err),
                }
            }
        });
    });
    group.finish();
}

criterion_group!(
    benches,
    bench_list,
    bench_nested,
    bench_dispatch,
    bench_hygiene,
    bench_identity,
    bench_budget
);
criterion_main!(benches);
