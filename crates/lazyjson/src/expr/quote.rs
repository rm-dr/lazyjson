/// Given a string beginning with a quote char (`'` or `"`), return the byte offset
/// just past the matching unescaped closing quote.
/// - Returns `s.len()` if the quote is never closed.
/// - Escaped quotes (`\'`) are handled correctly.
/// - The first char is assumed to be a quote.
pub fn scan_quoted(s: &str) -> usize {
	let mut chars = s.char_indices();

	let Some((_, quo)) = chars.next() else {
		return 0;
	};
	debug_assert!(
		quo == '\'' || quo == '"',
		"scan_quoted expects a leading quote, got {quo:?}",
	);

	let mut escape = false;
	for (i, c) in chars {
		if escape {
			escape = false;
			continue;
		}

		match c {
			'\\' => escape = true,
			_ if c == quo => return i + c.len_utf8(),
			_ => {}
		}
	}

	// Unterminated: the whole slice is the (open) string.
	s.len()
}

/// Strip one matching pair of surrounding quotes, if present.
pub fn strip_quotes(s: &str) -> &str {
	let bytes = s.as_bytes();
	if bytes.len() >= 2 {
		let first = bytes[0];
		let last = bytes[bytes.len() - 1];
		if (first == b'\'' || first == b'"') && first == last {
			return &s[1..s.len() - 1];
		}
	}
	s
}

/// Turn a raw string-literal token (including its surrounding quotes) into its
/// value: strips a matching pair of quotes, then resolves escape sequences.
pub fn unquote(s: &str) -> String {
	let inner = strip_quotes(s);

	let mut out = String::with_capacity(inner.len());
	let mut chars = inner.chars();
	while let Some(c) = chars.next() {
		if c != '\\' {
			out.push(c);
			continue;
		}

		match chars.next() {
			Some('n') => out.push('\n'),
			Some('t') => out.push('\t'),
			Some('r') => out.push('\r'),
			Some(other) => out.push(other),
			None => out.push('\\'),
		}
	}

	out
}

//
// MARK: tests
//

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn scan_simple() {
		assert_eq!(scan_quoted("'abc'"), 5);
		assert_eq!(scan_quoted("\"abc\""), 5);
	}

	#[test]
	fn scan_stops_at_first_closer() {
		// Only the first quoted run is scanned; the suffix is left for the caller.
		assert_eq!(scan_quoted("'a'.upper"), 3);
		assert_eq!(scan_quoted("'a'[0]"), 3);
	}

	#[test]
	fn scan_honors_escaped_quote() {
		// The escaped quote does not close the string.
		assert_eq!(scan_quoted(r"'a\'b'"), 6);
	}

	#[test]
	fn scan_unterminated_is_whole_slice() {
		assert_eq!(scan_quoted("'abc"), 4);
		assert_eq!(scan_quoted("'a\\"), 3);
	}

	#[test]
	fn scan_multibyte_escape_is_char_aligned() {
		// `\é` — the escaped char is two bytes; the offset must land on a char
		// boundary (a byte-based scanner would split it).
		let s = "'\\é'";
		let end = scan_quoted(s);
		assert!(s.is_char_boundary(end));
		assert_eq!(end, s.len());
	}

	#[test]
	fn unquote_resolves_escapes() {
		assert_eq!(unquote("'a\\tb'"), "a\tb");
		assert_eq!(unquote("'a\\\\b'"), "a\\b");
		assert_eq!(unquote("'foo\\''"), "foo'");
	}
}
