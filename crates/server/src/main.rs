//! `zebrafish` binary entry point.
//!
//! Skeleton: prints the startup banner so the release image has something to
//! run. The axum server, config plane, and CLI parsing land with WS-B.

fn main() {
    println!("{}", zebrafish_server::banner());
}
