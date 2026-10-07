//! Recursive macros, the recursion limit, and expansion backtraces.
//!
//! The driver expands to a fixed point, so a macro may expand into further
//! invocations — including of itself. Each invocation's name token carries the
//! context of the expansion that produced it, which is how the expander knows
//! how deeply nested a call is.
//!
//! - `count!` recurses on a shrinking argument list and terminates through its
//!   base case.
//! - `forever!` has no base case; the expander stops it at the configured
//!   recursion limit instead of looping, and `Expander::origin` walks the
//!   chain of expansions that led there.
//!
//! ```bash
//! cargo run --example recursion
//! ```

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::print_stdout)]

mod common;

use common::{Driver, Kind, define, parse_trees, render};
use intern_lang::Interner;
use macro_lang::{Context, ExpandError, Expander, Tree};

fn main() {
    let mut names = Interner::new();
    let count = define(
        "count",
        "
        () => { 0 } ;
        ($head:tt $($tail:tt)*) => { 1 + count!($($tail)*) }
        ",
        &mut names,
    );
    let forever = define("forever", "($x:tt) => { forever!($x) }", &mut names);

    let mut driver = Driver::new(Expander::with_limit(16));
    driver.add(count);
    driver.add(forever);

    let src = "count!(a b c d)";
    let mut program = parse_trees(src, &mut names);
    driver.expand_all(&mut program).expect("count! terminates");
    println!("{src} => {}", render(&program, &names, false));

    let src = "forever!(x)";
    let mut program = parse_trees(src, &mut names);
    match driver.expand_all(&mut program) {
        Err(ExpandError::RecursionLimit { limit }) => {
            println!("{src} => error: recursion limit of {limit} reached");
        }
        other => println!("unexpected: {other:?}"),
    }

    // The innermost `forever` token the driver left behind belongs to the
    // deepest expansion; follow `call_context` outward to build a backtrace.
    // Every nested call site is a span inside the macro's own definition; the
    // outermost one is the original invocation in `src`.
    let mut chain = Vec::new();
    let mut ctx = innermost_name_context(&program, &names);
    while let Some(origin) = driver.expander.origin(ctx) {
        chain.push(origin);
        ctx = origin.call_context;
    }
    for (depth, origin) in chain.iter().enumerate() {
        if depth < 2 || depth + 1 == chain.len() {
            println!(
                "  {:>2}: in `{}!`, invoked at {}",
                chain.len() - depth,
                names.resolve(origin.macro_name).unwrap_or("?"),
                origin.call_site
            );
        } else if depth == 2 {
            println!("      ...");
        }
    }
}

/// The context of the last `forever` identifier in the stream.
fn innermost_name_context(trees: &[Tree<Kind>], names: &Interner) -> Context {
    let mut found = Context::ROOT;
    for tree in trees {
        match tree {
            Tree::Token { token, ctx } => {
                if let Kind::Ident(sym) = token.kind {
                    if names.resolve(sym) == Some("forever") {
                        found = *ctx;
                    }
                }
            }
            Tree::Group { trees, .. } => {
                let inner = innermost_name_context(trees, names);
                if !inner.is_root() {
                    found = inner;
                }
            }
        }
    }
    found
}
