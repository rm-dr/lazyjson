use anyhow::{Result, bail};
use regex::Regex;
use tracing::warn;

#[derive(Debug)]
enum GlobSegment {
	/// A single segment
	Single(String),

	/// An optional doublestar segment
	DoubleStar,
}

impl GlobSegment {
	/// Returns the regex pattern of this part,
	/// prefixed with a /.
	fn to_regex_part(&self, prev: Option<&Self>, next: Option<&Self>) -> String {
		match (prev, self, next) {
			// Consecutive single segments need a trailing slash
			(_, Self::Single(x), Some(Self::Single(_))) => format!("{x}[/]"),

			// Terminal single segments don't need a trailing slash
			(_, Self::Single(x), None) => x.to_owned(),

			// Neighboring doublestar is always responsible for slashes
			(_, Self::Single(x), Some(Self::DoubleStar)) => x.to_owned(),

			// No additional slashes
			(None, Self::DoubleStar, None) => "((?:.*)?)".into(),

			// Leading slash
			(Some(Self::Single(_)), Self::DoubleStar, None) => "((?:[/].*)?)".into(),

			// Trailing slash
			(None, Self::DoubleStar, Some(Self::Single(_))) => "((?:.*[/])?)".into(),

			// Leading and trailing slash.
			// Also, replace self with a [/] when empty.
			(Some(Self::Single(_)), Self::DoubleStar, Some(Self::Single(_))) => {
				"((?:[/].*[/])|[/])".into()
			}

			// Doublestars cannot be neighbors
			(_, Self::DoubleStar, Some(Self::DoubleStar))
			| (Some(Self::DoubleStar), Self::DoubleStar, _) => {
				unreachable!("consecutive doublestars must be reduced")
			}
		}
	}
}

/// A path-matching glob pattern.
#[derive(Debug, Clone)]
pub struct GlobPattern {
	pub patterns: Vec<String>,
}

impl GlobPattern {
	pub fn regex(&self) -> Result<Regex> {
		// Flatten pattern
		// Double slashes are handled later
		let pattern = self.patterns.join("/");

		if pattern.ends_with("/") {
			warn!("Pattern `{pattern}` has a trailing slash which will be ignored")
		}

		if pattern.starts_with("/") {
			warn!("Pattern `{pattern}` has a leading slash which will be ignored")
		}

		// Split on slashes or stars
		// This is a lot like .split("/"), but handles
		// the edge case where ** is not delimited by slashes
		// (`root**test` is equivalent to `root/**/test`)
		let segments = {
			#[expect(clippy::unwrap_used)]
			let re = Regex::new("[*]{2,}|[/]").unwrap();
			let split = re.find_iter(&pattern);

			let bounds = split
				.into_iter()
				.flat_map(|x| {
					let r = x.range();
					let a = r.start;
					let b = r.end;
					[a, b]
				})
				.chain([pattern.len()])
				.collect::<Vec<_>>();

			let mut parts = Vec::new();
			let mut last = 0;
			for next in bounds {
				let seg = &pattern[last..next];
				// Consecutive slashes are identical to a single slash
				if seg != "/" && !seg.is_empty() {
					parts.push(seg);
				}
				last = next;
			}

			parts
		};

		let mut rebuilt_segments = Vec::new();
		let mut last_was_doublestar = false;
		for segment in segments {
			// This is a wildcard regex
			// (**, ***, etc)
			if segment.len() > 1 && segment.chars().all(|x| x == '*') {
				match segment {
					"**" => {
						// Consecutive doublestars are meaningless
						if !last_was_doublestar {
							rebuilt_segments.push(GlobSegment::DoubleStar);
						}
						last_was_doublestar = true;
					}
					_ => bail!("Invalid wildcard `{segment}`"),
				}
				continue;
			}
			last_was_doublestar = false;

			let parts = segment.split("*").collect::<Vec<_>>();

			let mut rebuilt = String::new();
			for (i, part) in parts.into_iter().enumerate() {
				if i != 0 {
					rebuilt.push_str("([^/]*)")
				}

				rebuilt.push_str(&regex::escape(part));
			}

			rebuilt_segments.push(GlobSegment::Single(rebuilt));
		}

		let mut re_built = String::new();
		let mut prev = None;
		for (i, seg) in rebuilt_segments.iter().enumerate() {
			let next = rebuilt_segments.get(i + 1);
			re_built.push_str(&seg.to_regex_part(prev, next));
			prev = Some(seg);
		}

		let re_built = format!("^{re_built}$");
		// This regex should always be valid
		#[expect(clippy::unwrap_used)]
		Ok(Regex::new(&re_built).unwrap())
	}
}

//
// MARK: tests
//

#[cfg(test)]
#[expect(clippy::unwrap_used)]
mod tests {
	use regex::Regex;

	use super::*;

	fn rule_regex(pattern: &[&str]) -> Regex {
		let rule = GlobPattern {
			patterns: pattern.iter().map(|x| x.to_string()).collect(),
		};

		return rule.regex().unwrap();
	}

	#[test]
	fn simple() {
		let regex = rule_regex(&["file.txt"]);

		assert!(regex.is_match("file.txt"));
		assert!(!regex.is_match("other.txt"));
		assert!(!regex.is_match("path/file.txt"));
	}

	#[test]
	fn simple_dir() {
		let regex = rule_regex(&["dir", "file.txt"]);

		assert!(regex.is_match("dir/file.txt"));
		assert!(!regex.is_match("file.txt"));
		assert!(!regex.is_match("other/file.txt"));
	}

	#[test]
	fn simple_star() {
		let regex = rule_regex(&["*.txt"]);

		assert!(regex.is_match("file.txt"));
		assert!(regex.is_match("other.txt"));
		assert!(!regex.is_match("file.jpg"));
		assert!(!regex.is_match("nested/file.txt"));
	}

	#[test]
	fn simple_doublestar() {
		let regex = rule_regex(&["**/*.txt"]);

		assert!(regex.is_match("file.txt"));
		assert!(regex.is_match("dir/file.txt"));
		assert!(regex.is_match("dir/subdir/file.txt"));
		assert!(!regex.is_match("file.jpg"));
		assert!(!regex.is_match("dir/file.jpg"));
	}

	#[test]
	fn consecutive_doublestar() {
		let regex = rule_regex(&["**", "**", "**", "*.txt"]);

		assert!(regex.is_match("file.txt"));
		assert!(regex.is_match("dir/file.txt"));
		assert!(regex.is_match("dir/subdir/file.txt"));
		assert!(!regex.is_match("file.jpg"));
		assert!(!regex.is_match("dir/file.jpg"));
	}

	#[test]
	fn dual_star() {
		let regex = rule_regex(&["**/*a*"]);

		assert!(regex.is_match("fileafile"));
		assert!(regex.is_match("dir/fileafile"));
		assert!(regex.is_match("filea"));
		assert!(regex.is_match("dir/filea"));
		assert!(regex.is_match("afile"));
		assert!(regex.is_match("dir/afile"));
		assert!(!regex.is_match("noletter"));
		assert!(!regex.is_match("dir/noletter"));
	}

	#[test]
	fn single_end() {
		let regex = rule_regex(&["**/*"]);

		assert!(regex.is_match("file"));
		assert!(regex.is_match("dir/file"));
		assert!(regex.is_match("a/b/c/dir/file"));
	}

	#[test]
	fn doublestar_end() {
		let regex = rule_regex(&["root/**"]);

		assert!(regex.is_match("root/file"));
		assert!(!regex.is_match("dir/file"));
	}

	#[test]
	fn doublestar_start() {
		let regex = rule_regex(&["**/dir"]);

		assert!(regex.is_match("dir"));
		assert!(regex.is_match("a/b/dir"));
		assert!(!regex.is_match("dir/file"));
	}

	#[test]
	fn doublestar_adjacent_before() {
		let regex = rule_regex(&["root/**test"]);

		assert!(regex.is_match("root/test"));
		assert!(regex.is_match("root/a/test"));
		assert!(regex.is_match("root/a/b/c/test"));
		assert!(!regex.is_match("root/file"));
		assert!(!regex.is_match("root/xxtest"));
	}

	#[test]
	fn doublestar_adjacent_after() {
		let regex = rule_regex(&["root/test**"]);

		assert!(regex.is_match("root/test"));
		assert!(regex.is_match("root/test/a"));
		assert!(regex.is_match("root/test/a/b/c"));
		assert!(!regex.is_match("root/testxx"));
		assert!(!regex.is_match("root/file"));
	}

	#[test]
	fn doublestar_adjacent_middle() {
		let regex = rule_regex(&["root/test**file"]);

		assert!(regex.is_match("root/test/file"));
		assert!(regex.is_match("root/test/a/b/c/file"));
		assert!(!regex.is_match("root/test"));
		assert!(!regex.is_match("root/file"));
		assert!(!regex.is_match("root/testfile"));
		assert!(!regex.is_match("root/testxxfile"));
	}

	#[test]
	fn doublestar_nullable() {
		let regex = rule_regex(&["root/**/file"]);

		assert!(regex.is_match("root/test/file"));
		assert!(regex.is_match("root/file"));
		assert!(!regex.is_match("rootfile"));
	}

	#[test]
	fn doublestar_nullable_post() {
		let regex = rule_regex(&["root/**"]);

		assert!(regex.is_match("root"));
		assert!(regex.is_match("root/file"));
		assert!(!regex.is_match("rootfile"));
	}

	#[test]
	fn doublestar_nullable_pre() {
		let regex = rule_regex(&["**/file"]);

		assert!(regex.is_match("file"));
		assert!(regex.is_match("root/file"));
		assert!(!regex.is_match("rootfile"));
	}

	#[test]
	fn doublestar_bad_extension() {
		let regex = rule_regex(&["**.flac"]);

		assert!(regex.is_match("root/.flac"));
		assert!(regex.is_match("root/a/.flac"));
		assert!(!regex.is_match("root/test.flac"));
		assert!(!regex.is_match("test.flac"));
		assert!(!regex.is_match("root/test/a/b/c.flac"));
		assert!(!regex.is_match("root/testflac"));
		assert!(!regex.is_match("test.mp3"));
	}

	#[test]
	fn doublestar_good_extension() {
		let regex = rule_regex(&["**/*.flac"]);

		assert!(regex.is_match("root/.flac"));
		assert!(regex.is_match("root/a/.flac"));
		assert!(regex.is_match("root/test.flac"));
		assert!(regex.is_match("test.flac"));
		assert!(regex.is_match("root/test/a/b/c.flac"));
		assert!(!regex.is_match("root/testflac"));
		assert!(!regex.is_match("test.mp3"));
	}

	#[test]
	fn multi_slash_a() {
		let regex = rule_regex(&["dir//file.txt"]);

		assert!(regex.is_match("dir/file.txt"));
		assert!(!regex.is_match("dirfile.txt"));
		assert!(!regex.is_match("dir/other.txt"));
	}

	#[test]
	fn multi_slash_b() {
		let regex = rule_regex(&["**///*.txt"]);

		assert!(regex.is_match("dir/file.txt"));
		assert!(regex.is_match("dir/subdir/file.txt"));
		assert!(!regex.is_match("file.jpg"));
	}

	#[test]
	fn multi_slash_c() {
		let regex = rule_regex(&["///dir//**//*.txt//"]);

		assert!(regex.is_match("dir/subdir/file.txt"));
		assert!(regex.is_match("dir/sub1/sub2/file.txt"));
		assert!(!regex.is_match("other/sub/file.txt"));
		assert!(!regex.is_match("dir/file.jpg"));
	}
}
