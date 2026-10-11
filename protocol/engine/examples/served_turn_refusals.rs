//! Prints the wire types of the client's `SERVED_TURN_REFUSALS`, sorted and
//! space-separated, read by the build's own reader from the reviewed client
//! patch named as the one argument.
//!
//! `scripts/check-served-refusals.sh` holds them to the types the backend
//! pins as the ones it serves. It compiles this file alone, with rustc, so it
//! uses nothing but the standard library and the reader.

#[path = "../served_turn_refusals.rs"]
#[allow(dead_code)]
mod served_turn_refusals;

fn main() {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let [patch] = arguments.as_slice() else {
        eprintln!("usage: served_turn_refusals PATCH");
        std::process::exit(2);
    };
    let patch = std::fs::read_to_string(patch).unwrap_or_else(|error| {
        eprintln!("read the reviewed client patch {patch}: {error}");
        std::process::exit(1);
    });
    let rows = served_turn_refusals::read(&patch).unwrap_or_else(|cause| {
        eprintln!("{cause}");
        std::process::exit(1);
    });
    let mut types: Vec<String> = rows.into_iter().map(|(wire, _)| wire).collect();
    types.sort();
    println!("{}", types.join(" "));
}
