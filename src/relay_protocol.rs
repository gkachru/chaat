//! Constants the app and the relay agree on. Both crates compile this file
//! (the relay through a `#[path]` module), so it must not depend on either.

/// The header a keyed relay expects the access key in.
pub const KEY_HEADER: &str = "x-chaat-relay-key";

/// Shorter access keys are too easy to guess.
pub const MIN_KEY_LEN: usize = 16;
