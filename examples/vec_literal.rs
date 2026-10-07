//! Rule selection and repetition: a `vec!`-style macro.
//!
//! Three rules, tried in order like the arms of `macro_rules!`:
//!
//! - `vec!()`            — an empty vector;
//! - `vec!(x; n)`        — `n` copies of an element;
//! - `vec!(a, b, c, ...)` — a list, expanded with a separated repetition.
//!
//! The last part shows what a failed match reports: the span of the token no
//! rule could get past, ready to underline in a diagnostic.
//!
//! ```bash
//! cargo run --example vec_literal
//! ```

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::print_stdout)]

mod common;

use common::{Driver, define, parse_trees, render};
use intern_lang::Interner;
use macro_lang::{ExpandError, Expander};

fn main() {
    let mut names = Interner::new();
    let vec = define(
        "vec",
        "
        () => { Vec::new() } ;
        ($elem:tt ; $n:tt) => { Vec::from_elem($elem, $n) } ;
        ($($x:tt),+) => {
            { let mut v = Vec::new() ; $( v.push($x) ; )* v }
        }
        ",
        &mut names,
    );

    let mut driver = Driver::new(Expander::new());
    driver.add(vec);

    for src in [
        "vec!()",
        "vec!(0; 16)",
        "vec!(1, 2, 3)",
        "vec!((a + b), [c], d)",
    ] {
        let mut program = parse_trees(src, &mut names);
        driver.expand_all(&mut program).expect("expansion");
        println!("{src:24} => {}", render(&program, &names, false));
    }

    // `vec!(1, 2,)` has a trailing comma the list rule does not allow.
    let src = "vec!(1, 2,)";
    let mut program = parse_trees(src, &mut names);
    match driver.expand_all(&mut program) {
        Err(ExpandError::NoMatch { span }) => {
            let start = span.start().to_usize();
            let width = span.len().max(1) as usize;
            println!();
            println!("error: no rules expected this token");
            println!("  {src}");
            println!("  {}{}", " ".repeat(start), "^".repeat(width));
        }
        other => println!("unexpected: {other:?}"),
    }
}
