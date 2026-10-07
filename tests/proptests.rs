//! Property tests holding the matcher and transcriber to an independent oracle.
//!
//! The matcher is an NFA simulation that merges parses and flags ambiguity.
//! The oracle here is the obvious, slow alternative: a dynamic program that
//! counts every distinct way a pattern can derive the input (capped at two).
//! For arbitrary patterns and inputs the expander must agree with it exactly —
//! no parse means `NoMatch`, one parse means success, two or more means
//! `Ambiguous` — and on success, a template that mirrors the pattern must
//! reproduce the input token for token.

#![allow(clippy::unwrap_used)]

use intern_lang::Symbol;
use macro_lang::{
    Context, ExpandError, Expander, Fragment, Kleene, Macro, MacroError, Pattern, Rule, Template,
    Tree,
};
use proptest::prelude::*;
use token_lang::{Span, Token};

/// Token kinds `0..TOKENS` are plain tokens; the delimiter pairs sit above.
const TOKENS: u8 = 4;
const PAREN: (u8, u8) = (10, 11);
const BRACKET: (u8, u8) = (12, 13);

fn is_even(kind: &u8) -> bool {
    kind % 2 == 0
}

// ---------------------------------------------------------------------------
// a pattern shape, before names are assigned
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
enum Shape {
    Tok(u8),
    AnyTree,
    Even,
    Group(bool, Vec<Shape>),
    Rep(Vec<Shape>, Option<u8>, Kleene),
}

fn kleene() -> impl Strategy<Value = Kleene> {
    prop_oneof![
        Just(Kleene::ZeroOrMore),
        Just(Kleene::OneOrMore),
        Just(Kleene::ZeroOrOne)
    ]
}

fn shape() -> impl Strategy<Value = Shape> {
    let leaf = prop_oneof![
        (0..TOKENS).prop_map(Shape::Tok),
        Just(Shape::AnyTree),
        Just(Shape::Even),
    ];
    leaf.prop_recursive(3, 20, 3, |inner| {
        prop_oneof![
            (any::<bool>(), prop::collection::vec(inner.clone(), 0..3))
                .prop_map(|(paren, body)| Shape::Group(paren, body)),
            (
                prop::collection::vec(inner, 1..3),
                prop::option::of(0..TOKENS),
                kleene()
            )
                .prop_map(|(body, sep, k)| Shape::Rep(body, sep, k)),
        ]
    })
}

/// Normalizes a shape so its mirror template is valid: `?` repetitions lose
/// their separator, and every repetition body binds at least one tree.
fn normalize(shape: Shape) -> Shape {
    match shape {
        Shape::Group(paren, body) => Shape::Group(paren, body.into_iter().map(normalize).collect()),
        Shape::Rep(body, sep, kleene) => {
            let mut body: Vec<Shape> = body.into_iter().map(normalize).collect();
            if !body.iter().any(binds) {
                body.push(Shape::AnyTree);
            }
            let sep = if kleene == Kleene::ZeroOrOne {
                None
            } else {
                sep
            };
            Shape::Rep(body, sep, kleene)
        }
        other => other,
    }
}

fn binds(shape: &Shape) -> bool {
    match shape {
        Shape::AnyTree | Shape::Even => true,
        Shape::Tok(_) => false,
        Shape::Group(_, body) | Shape::Rep(body, _, _) => body.iter().any(binds),
    }
}

fn delims(paren: bool) -> (u8, u8) {
    if paren { PAREN } else { BRACKET }
}

fn span() -> Span {
    Span::new(0, 1)
}

/// Lowers shapes to a pattern and its mirror template, naming binds 1, 2, ...
fn lower(shapes: &[Shape], next: &mut u32) -> (Vec<Pattern<u8>>, Vec<Template<u8>>) {
    let mut pattern = Vec::new();
    let mut template = Vec::new();
    for shape in shapes {
        match shape {
            Shape::Tok(kind) => {
                pattern.push(Pattern::Token(*kind));
                template.push(Template::token(*kind, span()));
            }
            Shape::AnyTree | Shape::Even => {
                let name = Symbol::from_u32(*next).unwrap();
                *next += 1;
                let fragment = if matches!(shape, Shape::Even) {
                    Fragment::Kind(is_even)
                } else {
                    Fragment::Tree
                };
                pattern.push(Pattern::Bind { name, fragment });
                template.push(Template::Var(name));
            }
            Shape::Group(paren, body) => {
                let (open, close) = delims(*paren);
                let (p, t) = lower(body, next);
                pattern.push(Pattern::Group {
                    open,
                    close,
                    body: p,
                });
                template.push(Template::Group {
                    open: Token::new(open, span()),
                    close: Token::new(close, span()),
                    body: t,
                });
            }
            Shape::Rep(body, sep, kleene) => {
                let (p, t) = lower(body, next);
                pattern.push(Pattern::Repeat {
                    body: p,
                    separator: *sep,
                    kleene: *kleene,
                });
                template.push(Template::Repeat {
                    body: t,
                    separator: sep.map(|s| Token::new(s, span())),
                });
            }
        }
    }
    (pattern, template)
}

// ---------------------------------------------------------------------------
// inputs
// ---------------------------------------------------------------------------

fn leaf(kind: u8) -> Tree<u8> {
    Tree::token(kind, span())
}

fn group(paren: bool, trees: Vec<Tree<u8>>) -> Tree<u8> {
    let (open, close) = delims(paren);
    Tree::Group {
        open: Token::new(open, span()),
        close: Token::new(close, span()),
        trees,
    }
}

fn random_trees() -> impl Strategy<Value = Vec<Tree<u8>>> {
    let leaf_tree = (0..TOKENS).prop_map(leaf);
    let tree = leaf_tree.prop_recursive(3, 24, 4, |inner| {
        (any::<bool>(), prop::collection::vec(inner, 0..4))
            .prop_map(|(paren, trees)| group(paren, trees))
    });
    prop::collection::vec(tree, 0..7)
}

/// A tiny deterministic generator for sampling inputs that match a shape.
struct Rng(u64);

impl Rng {
    fn below(&mut self, n: u64) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0 % n.max(1)
    }
}

/// Builds an input the shapes match (possibly also in other ways).
fn sample(shapes: &[Shape], rng: &mut Rng, out: &mut Vec<Tree<u8>>) {
    for shape in shapes {
        match shape {
            Shape::Tok(kind) => out.push(leaf(*kind)),
            Shape::Even => out.push(leaf(2 * rng.below(u64::from(TOKENS) / 2) as u8)),
            Shape::AnyTree => {
                if rng.below(4) == 0 {
                    out.push(group(rng.below(2) == 0, vec![leaf(rng.below(4) as u8)]));
                } else {
                    out.push(leaf(rng.below(u64::from(TOKENS)) as u8));
                }
            }
            Shape::Group(paren, body) => {
                let mut inner = Vec::new();
                sample(body, rng, &mut inner);
                out.push(group(*paren, inner));
            }
            Shape::Rep(body, sep, kleene) => {
                let count = match kleene {
                    Kleene::ZeroOrMore => rng.below(4),
                    Kleene::OneOrMore => 1 + rng.below(3),
                    Kleene::ZeroOrOne => rng.below(2),
                };
                for i in 0..count {
                    if i > 0 {
                        if let Some(sep) = sep {
                            out.push(leaf(*sep));
                        }
                    }
                    sample(body, rng, out);
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// the oracle
// ---------------------------------------------------------------------------

/// Derivation counts per input position, capped at 2 ("many").
type Counts = Vec<u8>;

fn add(slot: &mut u8, n: u8) {
    *slot = (*slot + n).min(2);
}

/// Counts derivations of `shapes` over `trees`, starting from `from`.
fn derive_seq(shapes: &[Shape], trees: &[Tree<u8>], from: Counts) -> Counts {
    shapes
        .iter()
        .fold(from, |counts, shape| derive_one(shape, trees, &counts))
}

fn derive_one(shape: &Shape, trees: &[Tree<u8>], from: &Counts) -> Counts {
    let mut out = vec![0u8; trees.len() + 1];
    for (pos, &ways) in from.iter().enumerate() {
        if ways == 0 {
            continue;
        }
        match shape {
            Shape::Rep(body, sep, kleene) => {
                let mut start = vec![0u8; trees.len() + 1];
                start[pos] = 1;
                let counts = derive_rep(body, *sep, *kleene, trees, start);
                for (end, n) in counts.into_iter().enumerate() {
                    add(&mut out[end], n.saturating_mul(ways).min(2));
                }
            }
            _ => {
                if let Some(n) = derive_tree(shape, trees.get(pos)) {
                    add(&mut out[pos + 1], n.saturating_mul(ways).min(2));
                }
            }
        }
    }
    out
}

/// Derivations of a single-tree shape over one tree: `None` or a count.
fn derive_tree(shape: &Shape, tree: Option<&Tree<u8>>) -> Option<u8> {
    match (shape, tree?) {
        (Shape::Tok(kind), Tree::Token { token, .. }) if token.kind == *kind => Some(1),
        (Shape::AnyTree, _) => Some(1),
        (Shape::Even, Tree::Token { token, .. }) if is_even(&token.kind) => Some(1),
        (Shape::Group(paren, body), Tree::Group { open, close, trees }) => {
            let (o, c) = delims(*paren);
            if open.kind != o || close.kind != c {
                return None;
            }
            let mut start = vec![0u8; trees.len() + 1];
            start[0] = 1;
            let n = derive_seq(body, trees, start)[trees.len()];
            (n > 0).then_some(n)
        }
        _ => None,
    }
}

fn derive_rep(
    body: &[Shape],
    sep: Option<u8>,
    kleene: Kleene,
    trees: &[Tree<u8>],
    start: Counts,
) -> Counts {
    let mut total = vec![0u8; trees.len() + 1];
    if kleene.allows(0) {
        for (t, s) in total.iter_mut().zip(&start) {
            add(t, *s);
        }
    }
    let mut current = derive_seq(body, trees, start);
    let mut iterations = 1;
    while current.iter().any(|&n| n > 0) {
        if kleene.allows(iterations) {
            for (t, c) in total.iter_mut().zip(&current) {
                add(t, *c);
            }
        }
        if kleene == Kleene::ZeroOrOne {
            break;
        }
        let mut next = vec![0u8; trees.len() + 1];
        for (pos, &n) in current.iter().enumerate() {
            if n == 0 {
                continue;
            }
            match sep {
                Some(sep) => {
                    if matches!(trees.get(pos), Some(Tree::Token { token, .. }) if token.kind == sep)
                    {
                        add(&mut next[pos + 1], n);
                    }
                }
                None => add(&mut next[pos], n),
            }
        }
        current = derive_seq(body, trees, next);
        iterations += 1;
    }
    total
}

fn count_parses(shapes: &[Shape], trees: &[Tree<u8>]) -> u8 {
    let mut start = vec![0u8; trees.len() + 1];
    start[0] = 1;
    derive_seq(shapes, trees, start)[trees.len()]
}

// ---------------------------------------------------------------------------
// properties
// ---------------------------------------------------------------------------

/// Every kind in the trees, delimiters included, depth first.
fn kinds(trees: &[Tree<u8>], out: &mut Vec<u8>) {
    for tree in trees {
        match tree {
            Tree::Token { token, .. } => out.push(token.kind),
            Tree::Group { open, close, trees } => {
                out.push(open.kind);
                kinds(trees, out);
                out.push(close.kind);
            }
        }
    }
}

fn flat_kinds(trees: &[Tree<u8>]) -> Vec<u8> {
    let mut out = Vec::new();
    kinds(trees, &mut out);
    out
}

fn contexts(trees: &[Tree<u8>], out: &mut Vec<Context>) {
    for tree in trees {
        match tree {
            Tree::Token { ctx, .. } => out.push(*ctx),
            Tree::Group { trees, .. } => contexts(trees, out),
        }
    }
}

/// Builds the macro for `shapes`, or `None` when a repetition body can match
/// nothing (which `Macro::new` must reject).
fn build(shapes: &[Shape]) -> Option<Macro<u8>> {
    let mut next = 1;
    let (pattern, template) = lower(shapes, &mut next);
    let name = Symbol::from_u32(10_000).unwrap();
    match Macro::new(name, vec![Rule { pattern, template }]) {
        Ok(mac) => Some(mac),
        Err(MacroError::EmptyRepetition { rule: 0 }) => None,
        Err(other) => panic!("mirror template rejected: {other:?}"),
    }
}

fn check_against_oracle(
    expander: &mut Expander<u8>,
    shapes: &[Shape],
    input: &[Tree<u8>],
) -> Result<(), TestCaseError> {
    let Some(mac) = build(shapes) else {
        return Ok(());
    };
    let parses = count_parses(shapes, input);
    let result = expander.expand(&mac, input, Span::new(0, 1), Context::ROOT);
    match (parses, result) {
        (0, Err(ExpandError::NoMatch { .. })) => {}
        (1, Ok(out)) => {
            prop_assert_eq!(flat_kinds(&out), flat_kinds(input));
            // Hygiene: the input is all root; at most one context is minted per
            // expansion, and it was minted from the root.
            let mut ctxs = Vec::new();
            contexts(&out, &mut ctxs);
            ctxs.retain(|c| !c.is_root());
            ctxs.dedup();
            prop_assert!(ctxs.len() <= 1);
            if let Some(ctx) = ctxs.first() {
                prop_assert_eq!(expander.origin(*ctx).unwrap().parent, Context::ROOT);
            }
        }
        (2, Err(ExpandError::Ambiguous { rule: 0 })) => {}
        (parses, other) => {
            return Err(TestCaseError::fail(format!(
                "oracle counted {parses} parse(s), expander returned {other:?}"
            )));
        }
    }
    Ok(())
}

fn shapes() -> impl Strategy<Value = Vec<Shape>> {
    prop::collection::vec(shape(), 0..4)
        .prop_map(|shapes| shapes.into_iter().map(normalize).collect())
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    /// Inputs sampled from the pattern itself: mostly matches, some ambiguous.
    #[test]
    fn agrees_with_oracle_on_sampled_inputs(shapes in shapes(), seed in 1u64..u64::MAX) {
        let mut rng = Rng(seed);
        let mut input = Vec::new();
        sample(&shapes, &mut rng, &mut input);
        check_against_oracle(&mut Expander::new(), &shapes, &input)?;
    }

    /// Arbitrary inputs: mostly mismatches, exercising every failure path.
    #[test]
    fn agrees_with_oracle_on_random_inputs(shapes in shapes(), input in random_trees()) {
        check_against_oracle(&mut Expander::new(), &shapes, &input)?;
    }

    /// One expander reused across many expansions behaves exactly like a fresh
    /// one each time: the pooled buffers carry no state between calls.
    #[test]
    fn reused_expander_matches_fresh_expanders(
        cases in prop::collection::vec((shapes(), 1u64..u64::MAX), 1..8)
    ) {
        let mut shared = Expander::new();
        for (shapes, seed) in &cases {
            let Some(mac) = build(shapes) else { continue };
            let mut input = Vec::new();
            sample(shapes, &mut Rng(*seed), &mut input);
            let reused = shared.expand(&mac, &input, Span::new(0, 1), Context::ROOT);
            let fresh = Expander::new().expand(&mac, &input, Span::new(0, 1), Context::ROOT);
            match (reused, fresh) {
                (Ok(a), Ok(b)) => prop_assert_eq!(flat_kinds(&a), flat_kinds(&b)),
                (a, b) => prop_assert_eq!(a.err(), b.err()),
            }
        }
    }

    /// `$($t:tt)*` is the identity: every tree comes back unchanged, spans and
    /// contexts included.
    #[test]
    fn tree_repetition_is_the_identity(input in random_trees()) {
        let t = Symbol::from_u32(1).unwrap();
        let mac = Macro::new(Symbol::from_u32(2).unwrap(), vec![Rule {
            pattern: vec![Pattern::Repeat {
                body: vec![Pattern::Bind { name: t, fragment: Fragment::Tree }],
                separator: None,
                kleene: Kleene::ZeroOrMore,
            }],
            template: vec![Template::Repeat { body: vec![Template::Var(t)], separator: None }],
        }]).unwrap();
        let out = Expander::new().expand(&mac, &input, Span::new(0, 1), Context::ROOT).unwrap();
        prop_assert_eq!(out, input);
    }
}
