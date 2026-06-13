//! Symbol cleanup: demangling + crate attribution.
//!
//! v0 leans on the symbol names the sampler already resolves (samply emits
//! `file:line` for many frames). Address -> `file:line` resolution via
//! `addr2line`/DWARF for frames the sampler left bare is a hack-day fan-out item.

use rustc_demangle::demangle;

/// Demangle a Rust symbol and strip the trailing hash (`::h1a2b...`).
pub fn demangle_symbol(raw: &str) -> String {
    format!("{:#}", demangle(raw))
}

/// Best-effort crate name from a demangled symbol.
///
/// Handles the common shapes:
/// - `polars_core::frame::DataFrame::sort` -> `polars_core`
/// - `<alloc::vec::Vec<T> as core::clone::Clone>::clone` -> `alloc`
/// - `core::ptr::drop_in_place<...>` -> `core`
/// - bare C symbols (`memcpy`) -> the symbol itself
pub fn crate_of(demangled: &str) -> String {
    let s = demangled.trim();
    // Step into a leading `<...>` qualifier to find the first real path.
    let s = s.strip_prefix('<').unwrap_or(s);
    let s = s.trim_start();

    // Take the leading identifier run (letters, digits, underscore).
    let mut ident = String::new();
    for ch in s.chars() {
        if ch.is_alphanumeric() || ch == '_' {
            ident.push(ch);
        } else {
            break;
        }
    }
    if ident.is_empty() {
        return "<unknown>".to_string();
    }
    // `impl`/`dyn` are never crate roots — fall back to the whole token.
    match ident.as_str() {
        "impl" | "dyn" => "<unknown>".to_string(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crate_extraction() {
        assert_eq!(crate_of("polars_core::frame::DataFrame::sort"), "polars_core");
        assert_eq!(
            crate_of("<alloc::vec::Vec<T> as core::clone::Clone>::clone"),
            "alloc"
        );
        assert_eq!(crate_of("memcpy"), "memcpy");
    }
}
