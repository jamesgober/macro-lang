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

use std::hint::black_box;

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use intern_lang::{Interner, Symbol};
use macro_lang::{Context, Expander, Fragment, Kleene, Macro, Pattern, Rule, Template, Tree};
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
        let mut expander = Expander::new();
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
        let mut expander = Expander::new();
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
    let mut expander = Expander::new();
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
    let mut expander = Expander::new();
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
    let mut expander = Expander::new();
    c.bench_function("identity/depth=64", |b| {
        b.iter(|| call(&mut expander, &mac, black_box(&input)));
    });
}

criterion_group!(
    benches,
    bench_list,
    bench_nested,
    bench_dispatch,
    bench_hygiene,
    bench_identity
);
criterion_main!(benches);
