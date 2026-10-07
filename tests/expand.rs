//! End-to-end expansion: definitions, matching, transcription, hygiene, and
//! every error path, driven through the public API only.
//!
//! Tokens are `char`s. `trees("a(b c)")` builds token trees from a string —
//! `()`, `[]`, and `{}` group, whitespace separates, every other character is
//! one token — and `render` prints trees back the same way, so each test reads
//! as source in, source out.

#![allow(clippy::unwrap_used)]

use intern_lang::{Interner, Symbol};
use macro_lang::{
    Context, ExpandError, Expander, Fragment, Kleene, Macro, MacroError, Pattern, Rule, Template,
    Tree,
};
use token_lang::{Span, Token};

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

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

/// Prints trees back to a compact string.
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

/// Every token's context, depth first.
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

/// Template tokens for every char of `src` (no groups).
fn lit(src: &str) -> Vec<Template<char>> {
    src.chars()
        .map(|c| Template::token(c, Span::new(500, 501)))
        .collect()
}

fn tgroup(open: char, body: Vec<Template<char>>) -> Template<char> {
    Template::Group {
        open: Token::new(open, Span::new(500, 501)),
        close: Token::new(closer(open).unwrap(), Span::new(500, 501)),
        body,
    }
}

fn pgroup(open: char, body: Vec<Pattern<char>>) -> Pattern<char> {
    Pattern::Group {
        open,
        close: closer(open).unwrap(),
        body,
    }
}

fn tt(name: Symbol) -> Pattern<char> {
    Pattern::Bind {
        name,
        fragment: Fragment::Tree,
    }
}

fn rep(body: Vec<Pattern<char>>, separator: Option<char>, kleene: Kleene) -> Pattern<char> {
    Pattern::Repeat {
        body,
        separator,
        kleene,
    }
}

fn trep(body: Vec<Template<char>>, separator: Option<char>) -> Template<char> {
    Template::Repeat {
        body,
        separator: separator.map(|c| Token::new(c, Span::new(500, 501))),
    }
}

struct Fixture {
    names: Interner,
    expander: Expander<char>,
}

impl Fixture {
    fn new() -> Self {
        Self {
            names: Interner::new(),
            expander: Expander::new(),
        }
    }

    fn sym(&mut self, name: &str) -> Symbol {
        self.names.intern(name)
    }

    fn define(&mut self, rules: Vec<Rule<char>>) -> Macro<char> {
        let name = self.sym("m");
        Macro::new(name, rules).unwrap()
    }

    fn expand(&mut self, mac: &Macro<char>, src: &str) -> Result<String, ExpandError> {
        let input = trees(src);
        let out = self
            .expander
            .expand(mac, &input, Span::new(0, 1000), Context::ROOT)?;
        Ok(render(&out))
    }
}

fn rule(pattern: Vec<Pattern<char>>, template: Vec<Template<char>>) -> Rule<char> {
    Rule { pattern, template }
}

// ---------------------------------------------------------------------------
// matching and transcription
// ---------------------------------------------------------------------------

#[test]
fn identity_macro_reproduces_input_exactly() {
    let mut f = Fixture::new();
    let t = f.sym("t");
    let mac = f.define(vec![rule(
        vec![rep(vec![tt(t)], None, Kleene::ZeroOrMore)],
        vec![trep(vec![Template::Var(t)], None)],
    )]);
    let input = trees("a (b [c d] {}) e");
    let out = f
        .expander
        .expand(&mac, &input, Span::new(0, 20), Context::ROOT)
        .unwrap();
    assert_eq!(out, input);
}

#[test]
fn literal_tokens_and_groups_must_appear_exactly() {
    let mut f = Fixture::new();
    let x = f.sym("x");
    let mac = f.define(vec![rule(
        vec![Pattern::Token('k'), pgroup('(', vec![tt(x)])],
        vec![Template::Var(x)],
    )]);
    assert_eq!(f.expand(&mac, "k(z)").unwrap(), "z");
    assert!(matches!(
        f.expand(&mac, "k[z]"),
        Err(ExpandError::NoMatch { .. })
    ));
    assert!(matches!(
        f.expand(&mac, "j(z)"),
        Err(ExpandError::NoMatch { .. })
    ));
    assert!(matches!(
        f.expand(&mac, "k(z w)"),
        Err(ExpandError::NoMatch { .. })
    ));
}

#[test]
fn first_matching_rule_wins() {
    let mut f = Fixture::new();
    let x = f.sym("x");
    let mac = f.define(vec![
        rule(vec![], lit("0")),
        rule(vec![Pattern::Token('a')], lit("A")),
        rule(vec![tt(x)], vec![Template::Var(x), Template::Var(x)]),
    ]);
    assert_eq!(f.expand(&mac, "").unwrap(), "0");
    assert_eq!(f.expand(&mac, "a").unwrap(), "A");
    assert_eq!(f.expand(&mac, "b").unwrap(), "bb");
    assert_eq!(f.expand(&mac, "(q)").unwrap(), "(q)(q)");
}

#[test]
fn separated_repetition() {
    let mut f = Fixture::new();
    let e = f.sym("e");
    let mac = f.define(vec![rule(
        vec![rep(vec![tt(e)], Some(','), Kleene::ZeroOrMore)],
        vec![tgroup('[', vec![trep(vec![Template::Var(e)], Some(';'))])],
    )]);
    assert_eq!(f.expand(&mac, "a,b,c").unwrap(), "[a;b;c]");
    assert_eq!(f.expand(&mac, "a").unwrap(), "[a]");
    assert_eq!(f.expand(&mac, "").unwrap(), "[]");
    // A trailing separator is not part of the pattern.
    assert!(matches!(
        f.expand(&mac, "a,b,"),
        Err(ExpandError::NoMatch { .. })
    ));
    // Neither is a missing one.
    assert!(matches!(
        f.expand(&mac, "a b"),
        Err(ExpandError::NoMatch { .. })
    ));
}

#[test]
fn kleene_bounds_are_enforced() {
    let mut f = Fixture::new();
    let x = f.sym("x");
    let plus = f.define(vec![rule(
        vec![rep(vec![tt(x)], None, Kleene::OneOrMore)],
        vec![trep(vec![Template::Var(x)], None)],
    )]);
    assert!(matches!(
        f.expand(&plus, ""),
        Err(ExpandError::NoMatch { .. })
    ));
    assert_eq!(f.expand(&plus, "abc").unwrap(), "abc");

    let y = f.sym("y");
    let opt = f.define(vec![rule(
        vec![
            rep(vec![tt(y)], None, Kleene::ZeroOrOne),
            Pattern::Token(';'),
        ],
        vec![trep(vec![Template::Var(y), Template::Var(y)], None)],
    )]);
    assert_eq!(f.expand(&opt, ";").unwrap(), "");
    assert_eq!(f.expand(&opt, "a;").unwrap(), "aa");
    assert!(matches!(
        f.expand(&opt, "ab;"),
        Err(ExpandError::NoMatch { .. })
    ));
}

#[test]
fn nested_repetitions_expand_in_order() {
    let mut f = Fixture::new();
    let x = f.sym("x");
    // $( ( $($x:tt)* ) )*  =>  $( [ $($x)* ] )*
    let mac = f.define(vec![rule(
        vec![rep(
            vec![pgroup(
                '(',
                vec![rep(vec![tt(x)], None, Kleene::ZeroOrMore)],
            )],
            None,
            Kleene::ZeroOrMore,
        )],
        vec![trep(
            vec![tgroup('[', vec![trep(vec![Template::Var(x)], None)])],
            None,
        )],
    )]);
    assert_eq!(f.expand(&mac, "(ab)(c)()").unwrap(), "[ab][c][]");
    assert_eq!(f.expand(&mac, "").unwrap(), "");
}

#[test]
fn shallow_variable_repeats_inside_deeper_repetition() {
    let mut f = Fixture::new();
    let head = f.sym("head");
    let x = f.sym("x");
    // $head:tt $($x:tt)*  =>  $( $head $x )*
    let mac = f.define(vec![rule(
        vec![tt(head), rep(vec![tt(x)], None, Kleene::ZeroOrMore)],
        vec![trep(vec![Template::Var(head), Template::Var(x)], None)],
    )]);
    assert_eq!(f.expand(&mac, "fabc").unwrap(), "fafbfc");
    assert_eq!(f.expand(&mac, "f").unwrap(), "");
}

#[test]
fn outer_variable_repeats_across_inner_repetition() {
    let mut f = Fixture::new();
    let name = f.sym("name");
    let field = f.sym("field");
    // $( $name:tt ( $($field:tt)* ) )*  =>  $( $( $name $field )* )*
    let mac = f.define(vec![rule(
        vec![rep(
            vec![
                tt(name),
                pgroup('(', vec![rep(vec![tt(field)], None, Kleene::ZeroOrMore)]),
            ],
            None,
            Kleene::ZeroOrMore,
        )],
        vec![trep(
            vec![trep(vec![Template::Var(name), Template::Var(field)], None)],
            None,
        )],
    )]);
    assert_eq!(f.expand(&mac, "p(xy) q(z) r()").unwrap(), "pxpyqz");
}

#[test]
fn lockstep_variables_must_agree() {
    let mut f = Fixture::new();
    let a = f.sym("a");
    let b = f.sym("b");
    // $($a:tt)* ; $($b:tt)*  =>  $( $a $b )*
    let mac = f.define(vec![rule(
        vec![
            rep(vec![tt(a)], None, Kleene::ZeroOrMore),
            Pattern::Token(';'),
            rep(vec![tt(b)], None, Kleene::ZeroOrMore),
        ],
        vec![trep(vec![Template::Var(a), Template::Var(b)], None)],
    )]);
    assert_eq!(f.expand(&mac, "ab;cd").unwrap(), "acbd");
    assert_eq!(
        f.expand(&mac, "ab;c"),
        Err(ExpandError::RepetitionMismatch { rule: 0 })
    );
}

#[test]
fn kind_fragment_restricts_captures() {
    let mut f = Fixture::new();
    let n = f.sym("n");
    let digits = f.define(vec![rule(
        vec![rep(
            vec![Pattern::Bind {
                name: n,
                fragment: Fragment::Kind(|c: &char| c.is_ascii_digit()),
            }],
            Some('+'),
            Kleene::OneOrMore,
        )],
        vec![trep(vec![Template::Var(n)], None)],
    )]);
    assert_eq!(f.expand(&digits, "1+2+3").unwrap(), "123");
    assert_eq!(
        f.expand(&digits, "1+x"),
        Err(ExpandError::NoMatch {
            span: Span::new(2, 3)
        })
    );
}

#[test]
fn trailing_literal_after_open_repetition() {
    let mut f = Fixture::new();
    let t = f.sym("t");
    // $($t:tt)* ;  =>  { $($t)* }
    let mac = f.define(vec![rule(
        vec![
            rep(vec![tt(t)], None, Kleene::ZeroOrMore),
            Pattern::Token(';'),
        ],
        vec![tgroup('{', vec![trep(vec![Template::Var(t)], None)])],
    )]);
    assert_eq!(f.expand(&mac, "a;b;").unwrap(), "{a;b}");
}

#[test]
fn deeply_nested_input_is_handled_iteratively() {
    let mut f = Fixture::new();
    let t = f.sym("t");
    let mac = f.define(vec![rule(
        vec![rep(vec![tt(t)], None, Kleene::ZeroOrMore)],
        vec![trep(vec![Template::Var(t)], None)],
    )]);
    let depth = 2_000;
    let src = format!("{}x{}", "(".repeat(depth), ")".repeat(depth));
    assert_eq!(f.expand(&mac, &src).unwrap(), src);
}

#[test]
fn long_input_matches() {
    let mut f = Fixture::new();
    let e = f.sym("e");
    let mac = f.define(vec![rule(
        vec![rep(vec![tt(e)], Some(','), Kleene::ZeroOrMore)],
        vec![trep(vec![Template::Var(e)], None)],
    )]);
    let src = vec!["a"; 5_000].join(",");
    assert_eq!(f.expand(&mac, &src).unwrap(), "a".repeat(5_000));
}

#[test]
fn adversarial_nesting_stays_linear() {
    // $( $( $x:tt )+ )+  has exponentially many parses of its input; a
    // backtracking matcher would never finish. The NFA merges them as it goes
    // and reports the ambiguity after one pass.
    let mut f = Fixture::new();
    let x = f.sym("x");
    let mac = f.define(vec![rule(
        vec![rep(
            vec![rep(vec![tt(x)], None, Kleene::OneOrMore)],
            None,
            Kleene::OneOrMore,
        )],
        vec![],
    )]);
    let src = "a".repeat(20_000);
    assert_eq!(
        f.expand(&mac, &src),
        Err(ExpandError::Ambiguous { rule: 0 })
    );
    // A single token has exactly one parse.
    assert_eq!(f.expand(&mac, "a").unwrap(), "");
}

// ---------------------------------------------------------------------------
// errors
// ---------------------------------------------------------------------------

#[test]
fn no_match_points_at_the_furthest_token() {
    let mut f = Fixture::new();
    let mac = f.define(vec![
        rule(vec![Pattern::Token('a'), Pattern::Token('b')], vec![]),
        rule(
            vec![
                Pattern::Token('a'),
                Pattern::Token('c'),
                Pattern::Token('d'),
            ],
            vec![],
        ),
    ]);
    // The second rule gets furthest: past `a c`, stuck at `x`.
    assert_eq!(
        f.expand(&mac, "acx"),
        Err(ExpandError::NoMatch {
            span: Span::new(2, 3)
        })
    );
    // Input ending early: an empty span just past the last token.
    assert_eq!(
        f.expand(&mac, "ac"),
        Err(ExpandError::NoMatch {
            span: Span::empty(2)
        })
    );
    // Empty input against rules that need tokens: the call site.
    assert_eq!(
        f.expand(&mac, ""),
        Err(ExpandError::NoMatch {
            span: Span::new(0, 1000)
        })
    );
}

#[test]
fn ambiguous_rule_stops_the_search() {
    let mut f = Fixture::new();
    let a = f.sym("a");
    let b = f.sym("b");
    let mac = f.define(vec![
        rule(
            vec![
                rep(vec![tt(a)], None, Kleene::ZeroOrMore),
                rep(vec![tt(b)], None, Kleene::ZeroOrMore),
            ],
            vec![],
        ),
        rule(vec![Pattern::Token('x'), Pattern::Token('y')], lit("never")),
    ]);
    assert_eq!(
        f.expand(&mac, "xy"),
        Err(ExpandError::Ambiguous { rule: 0 })
    );
}

#[test]
fn definition_errors() {
    let mut f = Fixture::new();
    let x = f.sym("x");
    let name = f.sym("bad");
    let check = |rules: Vec<Rule<char>>| Macro::new(name, rules).unwrap_err();

    assert_eq!(check(vec![]), MacroError::NoRules);
    assert_eq!(
        check(vec![rule(vec![tt(x), tt(x)], vec![])]),
        MacroError::DuplicateBinding { rule: 0, name: x }
    );
    assert_eq!(
        check(vec![rule(vec![], vec![Template::Var(x)])]),
        MacroError::UnboundVariable { rule: 0, name: x }
    );
    assert_eq!(
        check(vec![rule(
            vec![rep(vec![tt(x)], None, Kleene::ZeroOrMore)],
            vec![Template::Var(x)]
        )]),
        MacroError::StillRepeating { rule: 0, name: x }
    );
    assert_eq!(
        check(vec![rule(vec![tt(x)], vec![trep(lit("z"), None)])]),
        MacroError::NoRepeatingVariable { rule: 0 }
    );
    assert_eq!(
        check(vec![rule(
            vec![rep(vec![], None, Kleene::OneOrMore)],
            vec![]
        )]),
        MacroError::EmptyRepetition { rule: 0 }
    );
    assert_eq!(
        check(vec![rule(
            vec![rep(vec![tt(x)], Some(','), Kleene::ZeroOrOne)],
            vec![]
        )]),
        MacroError::OptionalSeparator { rule: 0 }
    );
}

// ---------------------------------------------------------------------------
// hygiene
// ---------------------------------------------------------------------------

#[test]
fn template_tokens_get_one_fresh_context_per_expansion() {
    let mut f = Fixture::new();
    let x = f.sym("x");
    let mac = f.define(vec![rule(
        vec![tt(x)],
        vec![
            Template::token('t', Span::new(50, 51)),
            Template::token('=', Span::new(52, 53)),
            Template::Var(x),
        ],
    )]);

    let first = f
        .expander
        .expand(&mac, &trees("t"), Span::new(0, 1), Context::ROOT)
        .unwrap();
    let ctx = contexts(&first);
    assert_eq!(ctx[0].1, ctx[1].1, "one context per expansion");
    assert!(!ctx[0].1.is_root());
    assert_eq!(
        ctx[2],
        ('t', Context::ROOT),
        "captured token keeps its context"
    );

    let second = f
        .expander
        .expand(&mac, &trees("t"), Span::new(9, 10), Context::ROOT)
        .unwrap();
    assert_ne!(
        contexts(&second)[0].1,
        ctx[0].1,
        "each expansion mints its own context"
    );
}

#[test]
fn separators_and_group_contents_are_marked() {
    let mut f = Fixture::new();
    let e = f.sym("e");
    let mac = f.define(vec![rule(
        vec![rep(vec![tt(e)], None, Kleene::ZeroOrMore)],
        vec![tgroup(
            '(',
            vec![
                Template::token('k', Span::new(0, 1)),
                trep(vec![Template::Var(e)], Some(',')),
            ],
        )],
    )]);
    let out = f
        .expander
        .expand(&mac, &trees("ab"), Span::new(0, 2), Context::ROOT)
        .unwrap();
    assert_eq!(render(&out), "(ka,b)");
    let ctx = contexts(&out);
    let minted = ctx[0].1;
    assert!(!minted.is_root());
    assert_eq!(
        ctx,
        vec![
            ('k', minted),
            ('a', Context::ROOT),
            (',', minted),
            ('b', Context::ROOT)
        ]
    );
}

#[test]
fn captured_tokens_keep_contexts_from_earlier_expansions() {
    let mut f = Fixture::new();
    let x = f.sym("x");
    let inner = f.define(vec![rule(vec![], lit("v"))]);
    let wrap = f.define(vec![rule(
        vec![tt(x)],
        vec![tgroup('(', vec![Template::Var(x)])],
    )]);

    let produced = f
        .expander
        .expand(&inner, &[], Span::new(0, 1), Context::ROOT)
        .unwrap();
    let v_ctx = contexts(&produced)[0].1;

    let wrapped = f
        .expander
        .expand(&wrap, &produced, Span::new(0, 5), Context::ROOT)
        .unwrap();
    assert_eq!(contexts(&wrapped), vec![('v', v_ctx)]);
}

#[test]
fn origin_describes_the_expansion() {
    let mut f = Fixture::new();
    let mac_name = f.sym("make");
    let mac = Macro::new(mac_name, vec![rule(vec![], lit("q"))]).unwrap();
    let out = f
        .expander
        .expand(&mac, &[], Span::new(4, 12), Context::ROOT)
        .unwrap();
    let ctx = contexts(&out)[0].1;
    let origin = f.expander.origin(ctx).unwrap();
    assert_eq!(origin.macro_name, mac_name);
    assert_eq!(origin.call_site, Span::new(4, 12));
    assert_eq!(origin.parent, Context::ROOT);
    assert_eq!(origin.call_context, Context::ROOT);
    assert_eq!(f.expander.origin(Context::ROOT), None);
}

#[test]
fn macro_defined_by_expansion_keeps_definition_context() {
    let mut f = Fixture::new();
    // An outer expansion produced the token `d` with some context; a macro
    // defined from that output carries the context into its template.
    let outer = f.define(vec![rule(vec![], lit("d"))]);
    let produced = f
        .expander
        .expand(&outer, &[], Span::new(0, 1), Context::ROOT)
        .unwrap();
    let def_ctx = contexts(&produced)[0].1;

    let name = f.sym("defined");
    let inner = Macro::new(
        name,
        vec![rule(
            vec![],
            vec![Template::Token {
                token: Token::new('d', Span::new(0, 1)),
                ctx: def_ctx,
            }],
        )],
    )
    .unwrap();
    let out = f
        .expander
        .expand(&inner, &[], Span::new(20, 21), Context::ROOT)
        .unwrap();
    let ctx = contexts(&out)[0].1;
    assert_ne!(ctx, def_ctx);
    assert_eq!(f.expander.origin(ctx).unwrap().parent, def_ctx);
}

#[test]
fn failed_expansion_leaves_no_trace() {
    let mut f = Fixture::new();
    let a = f.sym("a");
    let b = f.sym("b");
    let mac = f.define(vec![rule(
        vec![
            rep(vec![tt(a)], None, Kleene::ZeroOrMore),
            Pattern::Token(';'),
            rep(vec![tt(b)], None, Kleene::ZeroOrMore),
        ],
        // A literal token is written (and its context minted) before the
        // lockstep failure is discovered.
        vec![
            Template::token('k', Span::new(0, 1)),
            trep(vec![Template::Var(a), Template::Var(b)], None),
        ],
    )]);
    assert_eq!(
        f.expand(&mac, "a;bc"),
        Err(ExpandError::RepetitionMismatch { rule: 0 })
    );
    assert_eq!(
        format!("{:?}", f.expander),
        "Expander { limit: 128, contexts: 0, expansions: 0 }"
    );
    let ok = f
        .expander
        .expand(&mac, &trees("a;b"), Span::new(0, 3), Context::ROOT)
        .unwrap();
    assert_eq!(
        contexts(&ok)[0].1.as_u32(),
        1,
        "the first context is reused"
    );
}

// ---------------------------------------------------------------------------
// recursion
// ---------------------------------------------------------------------------

/// A minimal expansion driver: repeatedly finds `!` followed by a group (the
/// invocation of the single macro under test, written `!( ... )`) and splices
/// in its expansion, passing the context of the `!` token as the call context.
fn drive(
    expander: &mut Expander<char>,
    mac: &Macro<char>,
    src: &str,
) -> Result<String, ExpandError> {
    let mut stream = trees(src);
    loop {
        let found = stream.windows(2).position(|pair| {
            matches!(&pair[0], Tree::Token { token, .. } if token.kind == '!')
                && matches!(&pair[1], Tree::Group { open, .. } if open.kind == '(')
        });
        let Some(at) = found else {
            return Ok(render(&stream));
        };
        let (Tree::Token { token, ctx }, Tree::Group { trees: args, .. }) =
            (&stream[at], &stream[at + 1])
        else {
            unreachable!("matched above");
        };
        let out = expander.expand(mac, args, token.span, *ctx)?;
        let _invocation: Vec<_> = stream.splice(at..at + 2, out).collect();
    }
}

#[test]
fn recursive_macro_terminates_through_its_base_case() {
    let mut f = Fixture::new();
    let x = f.sym("x");
    let rest = f.sym("rest");
    // count!()           => 0
    // count!($x $($rest)*) => 1 + count!($($rest)*)
    let count = f.define(vec![
        rule(vec![], lit("0")),
        rule(
            vec![tt(x), rep(vec![tt(rest)], None, Kleene::ZeroOrMore)],
            vec![
                Template::token('1', Span::new(0, 1)),
                Template::token('+', Span::new(0, 1)),
                Template::token('!', Span::new(0, 1)),
                tgroup('(', vec![trep(vec![Template::Var(rest)], None)]),
            ],
        ),
    ]);
    assert_eq!(drive(&mut f.expander, &count, "!(abc)").unwrap(), "1+1+1+0");
}

#[test]
fn runaway_recursion_hits_the_limit() {
    let mut f = Fixture::new();
    f.expander = Expander::with_limit(8);
    // forever!() => forever!()
    let forever = f.define(vec![rule(
        vec![],
        vec![Template::token('!', Span::new(0, 1)), tgroup('(', vec![])],
    )]);
    assert_eq!(
        drive(&mut f.expander, &forever, "!()"),
        Err(ExpandError::RecursionLimit { limit: 8 })
    );
    // Eight levels were expanded before the ninth was refused.
    assert_eq!(
        format!("{:?}", f.expander),
        "Expander { limit: 8, contexts: 8, expansions: 8 }"
    );
}

#[test]
fn zero_limit_rejects_every_expansion() {
    let mut f = Fixture::new();
    f.expander = Expander::with_limit(0);
    let mac = f.define(vec![rule(vec![], vec![])]);
    assert_eq!(
        f.expand(&mac, ""),
        Err(ExpandError::RecursionLimit { limit: 0 })
    );
}
