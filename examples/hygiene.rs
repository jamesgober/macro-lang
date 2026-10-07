//! Hygiene: why a macro's temporary cannot capture the caller's variable.
//!
//! The classic failure of an unhygienic macro system is `swap!`. Its template
//! introduces a temporary, `tmp`; if the caller happens to swap a variable that
//! is *also* called `tmp`, a textual expansion silently produces
//! `let tmp = tmp; tmp = x; x = tmp;` — which swaps nothing.
//!
//! With macro-lang every identifier the template introduces carries a context
//! minted for that expansion. The output prints contexts as `name#n`: the
//! macro's `tmp#1` and the caller's `tmp` are different bindings to any
//! resolver that compares names *and* contexts.
//!
//! ```bash
//! cargo run --example hygiene
//! ```

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::print_stdout)]

mod common;

use common::{Driver, Kind, define, parse_trees, render};
use intern_lang::Interner;
use macro_lang::{Context, Expander, Tree};

fn main() {
    let mut names = Interner::new();
    let swap = define(
        "swap",
        "($a:ident, $b:ident) => { let tmp = $a ; $a = $b ; $b = tmp ; }",
        &mut names,
    );

    let mut driver = Driver::new(Expander::new());
    driver.add(swap);

    for src in ["swap!(x, y)", "swap!(tmp, x)"] {
        let mut program = parse_trees(src, &mut names);
        driver.expand_all(&mut program).expect("expansion");
        println!("{src}");
        println!("  => {}", render(&program, &names, true));
    }

    // A resolver sees the difference directly: the two `tmp`s in the second
    // expansion have the same symbol but different contexts.
    let mut program = parse_trees("swap!(tmp, x)", &mut names);
    driver.expand_all(&mut program).expect("expansion");
    let tmp = names.intern("tmp");
    let mut contexts: Vec<Context> = Vec::new();
    collect_contexts(&program, Kind::Ident(tmp), &mut contexts);
    contexts.dedup();
    println!();
    println!(
        "`tmp` appears with {} distinct contexts: {}",
        contexts.len(),
        contexts
            .iter()
            .map(|ctx| match driver.expander.origin(*ctx) {
                Some(origin) => format!(
                    "{ctx} (introduced by `{}!`)",
                    names.resolve(origin.macro_name).unwrap_or("?")
                ),
                None => format!("{ctx} (written by the caller)"),
            })
            .collect::<Vec<_>>()
            .join(", ")
    );
}

fn collect_contexts(trees: &[Tree<Kind>], kind: Kind, out: &mut Vec<Context>) {
    for tree in trees {
        match tree {
            Tree::Token { token, ctx } if token.kind == kind => {
                if !out.contains(ctx) {
                    out.push(*ctx);
                }
            }
            Tree::Token { .. } => {}
            Tree::Group { trees, .. } => collect_contexts(trees, kind, out),
        }
    }
}
