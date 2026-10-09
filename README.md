# LazyJson

Lazily computed, asynchronous value extraction from structured data.

## 🔧 TODO

-   Dynamic operator definitions
-   Dynamic field definitions
-   Completion api
-   Docs
-   Report error with fancy position

## Features

All are disabled by default.

-   `fs`: enable extra extractors for files
-   `hash`: enable hash extractors (sha1, sha2, blake3, md5)
-   `toml`: enable toml parsing (strings and blobs)
-   `id3`: enable id3 tag extraction from binary blobs
-   `exif`: enable exif extraction from images (via `kamadak-exif`)
-   `image`: enable image processing (resize, cropping)
-   `template`: enable templating (see below)

## Basic usage

Parse an expression, evaluate it against a root value, and serialize the result:

```rust
use lazyjson::{
	expr::{eval::ExpressionEvaluator, tree::LazyExpressionTree},
	value::{ELazyValue, LazyValue, extract::ExtractState},
};
use serde_json::json;

let root: ELazyValue = LazyValue::from(json!({
	"name": "  Alexei Stukov  ",
	"score": 0.82,
	"tags": ["sc1", "sc2", "zerg", "terran"],
	"path": "src/expr/tree.rs",
	"nickname": null,
}))
.into();

let state = ExtractState::default();
let tree = LazyExpressionTree::parse("@.name.trim.split( )[-1]")?;

let value = ExpressionEvaluator { root: &root, state: &state }
	.eval(&tree)
	.await?;

assert_eq!(value.to_json(&state).await?, Some(json!("Stukov")));
```

Expressions never panic on missing data — a path that resolves to nothing is
`null`, which `|` turns into a fallback:

| Expression                         | Result                 |
| ---------------------------------- | ---------------------- |
| `@.name.trim`                      | `"Ada Stukov"`         |
| `@.name.trim.lower`                | `"alexei stukov"`      |
| `@.name.trim.trimprefix(Alexei )`  | `"Stukov"`             |
| `@.name.trim.split( )`             | `["Alexei", "Stukov"]` |
| `@.path.split(/)[-1]`              | `"tree.rs"`            |
| `@.tags[0]`                        | `"sc1"`                |
| `@.tags[-1]`                       | `"terran"`             |
| `@.tags[2..]`                      | `["zerg", "terran"]`   |
| `@.tags[0..=1]`                    | `["sc1", "sc2"]`       |
| `@.tags.len`                       | `3`                    |
| `@.nickname.nonempty`              | `null`                 |
| `@.missing \| 'anon'`              | `"anon"`               |
| `@.score >= 0.7 && @.tags.len > 2` | `true`                 |
| `@.path ~= 'tree'`                 | `true`                 |
| `@.path ~=/ '**/*.rs'`             | `true`                 |
| `@.score >= 0.7 => 'high'`         | `"high"`               |
| `'hello'.upper`                    | `"HELLO"`              |

## Templates

LazyJson also provides a templating language similar to [minijinja](https://github.com/mitsuhiko/minijinja), gated behind the `template` feature:

```rust
use lazyjson::{
	template::Template,
	value::{ELazyValue, LazyValue, extract::ExtractState},
};
use serde_json::json;

let context: ELazyValue = LazyValue::from(json!({
	"project": "lazyjson",
	"tags": ["async", "lazy", "json"],
	"files": [
		{ "path": "src/lib.rs",       "lines": 120 },
		{ "path": "src/expr/eval.rs", "lines": 310 },
		{ "path": "README.md",        "lines": 40 }
	],
}))
.into();

let template = Template::parse(
	"\
# {{ @.project.upper }}

tags: {{ for t in @.tags }}{{ t }} {{ endfor }}

{{ let big = 200 }}\
{{ for f in @.files }}\
{{ let name = f.path.split(/)[-1] }}\
- {{ name }} ({{ f.lines }} lines){{ if f.lines >= big }} <- big{{ endif }}
{{ endfor }}\
{{ if @.files.len > 2 }}{{ @.files.len }} files total{{ else }}small project{{ endif }}
",
)?;

let out = template.expand(&ExtractState::default(), context).await?;
```

which renders:

```text
# LAZYJSON

tags: async lazy json

- lib.rs (120 lines)
- eval.rs (310 lines) <- big
- README.md (40 lines)
3 files total
```
