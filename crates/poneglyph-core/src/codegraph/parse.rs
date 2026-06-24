//! Tree-sitter parsing: walk each file's syntax tree once, extracting
//! symbols (nodes) plus raw call/test references that get resolved against
//! the whole graph in a second pass (`build.rs`) once every file in the
//! build is parsed.
//!
//! Deliberately a manual recursive walk over `Node`/`child_by_field_name`
//! rather than the `Query` DSL: one walker plus a per-language kind table
//! covers 15 grammars without juggling 15 sets of S-expression patterns.

use anyhow::{Result, bail};
use tree_sitter::{Language, Node, Parser};

use crate::model::{CgNode, CgNodeKind};

pub struct ParsedFile {
    pub nodes: Vec<CgNode>,
    /// (caller_node_id, callee_name) — caller is `None` for calls at module
    /// top level (rare, still worth resolving where possible).
    pub calls: Vec<(Option<String>, String)>,
    /// (test_node_id, guessed_target_name) — only populated where the
    /// language's naming convention makes the guess reliable (python
    /// `test_x` → `x`, go `TestX` → `X`); empty guess means "skip".
    pub tests: Vec<(String, Option<String>)>,
    /// (subtype_name, supertype_name) — class extends/interface implements/
    /// Rust trait impl, name-based like `calls`/`tests`, resolved in pass 2.
    pub inherits: Vec<(String, String)>,
}

struct LangSpec {
    function_kinds: &'static [&'static str],
    type_kinds: &'static [&'static str],
    import_kinds: &'static [&'static str],
    call_kinds: &'static [&'static str],
    /// Node kinds (children of a `type_kinds` node) that hold extends/
    /// implements/superclass identifiers — scanned generically for
    /// identifier-like descendant tokens. Languages whose heritage clause is
    /// better reached via a named field (python's `superclasses`, scala's
    /// `extend`) leave this empty and are special-cased in `walk()` instead.
    heritage_kinds: &'static [&'static str],
}

/// One row per supported language — the single source of truth for
/// extension routing, grammar loading, and node-kind tables. To add a
/// language: add its `tree-sitter-<lang>` dep to both `Cargo.toml`s, add a
/// row here (and a `guess_test_target` arm below if it has a test-naming
/// convention worth detecting). No other match statement to keep in sync.
struct LangEntry {
    name: &'static str,
    extensions: &'static [&'static str],
    grammar: fn() -> Language,
    spec: LangSpec,
}

fn lang_rust() -> Language {
    tree_sitter_rust::LANGUAGE.into()
}
fn lang_typescript() -> Language {
    tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()
}
fn lang_javascript() -> Language {
    tree_sitter_javascript::LANGUAGE.into()
}
fn lang_python() -> Language {
    tree_sitter_python::LANGUAGE.into()
}
fn lang_go() -> Language {
    tree_sitter_go::LANGUAGE.into()
}
fn lang_c() -> Language {
    tree_sitter_c::LANGUAGE.into()
}
fn lang_cpp() -> Language {
    tree_sitter_cpp::LANGUAGE.into()
}
fn lang_java() -> Language {
    tree_sitter_java::LANGUAGE.into()
}
fn lang_csharp() -> Language {
    tree_sitter_c_sharp::LANGUAGE.into()
}
fn lang_php() -> Language {
    tree_sitter_php::LANGUAGE_PHP.into()
}
fn lang_ruby() -> Language {
    tree_sitter_ruby::LANGUAGE.into()
}
fn lang_kotlin() -> Language {
    tree_sitter_kotlin_ng::LANGUAGE.into()
}
fn lang_swift() -> Language {
    tree_sitter_swift::LANGUAGE.into()
}
fn lang_bash() -> Language {
    tree_sitter_bash::LANGUAGE.into()
}
fn lang_scala() -> Language {
    tree_sitter_scala::LANGUAGE.into()
}

static LANGS: &[LangEntry] = &[
    LangEntry {
        name: "rust",
        extensions: &["rs"],
        grammar: lang_rust,
        spec: LangSpec {
            function_kinds: &["function_item"],
            type_kinds: &["struct_item", "enum_item", "trait_item"],
            import_kinds: &["use_declaration"],
            call_kinds: &["call_expression", "macro_invocation"],
            heritage_kinds: &[], // no classical inheritance — `impl_item` trait impls are special-cased in walk()
        },
    },
    LangEntry {
        name: "typescript",
        extensions: &["ts", "tsx"],
        grammar: lang_typescript,
        spec: LangSpec {
            function_kinds: &["function_declaration", "method_definition"],
            type_kinds: &["class_declaration", "interface_declaration"],
            import_kinds: &["import_statement"],
            call_kinds: &["call_expression"],
            heritage_kinds: &["class_heritage", "extends_type_clause"],
        },
    },
    LangEntry {
        name: "javascript",
        extensions: &["js", "jsx", "mjs", "cjs"],
        grammar: lang_javascript,
        spec: LangSpec {
            function_kinds: &["function_declaration", "method_definition"],
            type_kinds: &["class_declaration", "interface_declaration"],
            import_kinds: &["import_statement"],
            call_kinds: &["call_expression"],
            heritage_kinds: &["class_heritage"],
        },
    },
    LangEntry {
        name: "python",
        extensions: &["py"],
        grammar: lang_python,
        spec: LangSpec {
            function_kinds: &["function_definition"],
            type_kinds: &["class_definition"],
            import_kinds: &["import_statement", "import_from_statement"],
            call_kinds: &["call"],
            heritage_kinds: &[], // reached via the `superclasses` field, special-cased in walk()
        },
    },
    LangEntry {
        name: "go",
        extensions: &["go"],
        grammar: lang_go,
        spec: LangSpec {
            function_kinds: &["function_declaration", "method_declaration"],
            type_kinds: &["type_spec"],
            import_kinds: &["import_declaration"],
            call_kinds: &["call_expression"],
            heritage_kinds: &[], // no inheritance concept
        },
    },
    LangEntry {
        name: "c",
        extensions: &["c", "h"],
        grammar: lang_c,
        spec: LangSpec {
            function_kinds: &["function_definition"],
            type_kinds: &["struct_specifier", "enum_specifier", "type_definition"],
            import_kinds: &["preproc_include"],
            call_kinds: &["call_expression"],
            heritage_kinds: &[], // no inheritance concept
        },
    },
    LangEntry {
        name: "cpp",
        extensions: &["cpp", "cc", "cxx", "hpp", "hxx", "hh"],
        grammar: lang_cpp,
        spec: LangSpec {
            function_kinds: &["function_definition"],
            type_kinds: &["struct_specifier", "enum_specifier", "class_specifier", "type_definition"],
            import_kinds: &["preproc_include", "using_declaration"],
            call_kinds: &["call_expression"],
            heritage_kinds: &["base_class_clause"],
        },
    },
    LangEntry {
        name: "java",
        extensions: &["java"],
        grammar: lang_java,
        spec: LangSpec {
            function_kinds: &["method_declaration", "constructor_declaration"],
            type_kinds: &["class_declaration", "interface_declaration", "enum_declaration"],
            import_kinds: &["import_declaration"],
            call_kinds: &["method_invocation", "object_creation_expression"],
            heritage_kinds: &["superclass", "super_interfaces", "extends_interfaces"],
        },
    },
    LangEntry {
        name: "csharp",
        extensions: &["cs"],
        grammar: lang_csharp,
        spec: LangSpec {
            function_kinds: &["method_declaration", "constructor_declaration"],
            type_kinds: &["class_declaration", "interface_declaration", "struct_declaration", "enum_declaration"],
            import_kinds: &["using_directive"],
            call_kinds: &["invocation_expression", "object_creation_expression"],
            heritage_kinds: &["base_list"],
        },
    },
    LangEntry {
        name: "php",
        extensions: &["php"],
        grammar: lang_php,
        spec: LangSpec {
            function_kinds: &["function_definition", "method_declaration"],
            type_kinds: &["class_declaration", "interface_declaration", "enum_declaration"],
            import_kinds: &["namespace_use_declaration"],
            call_kinds: &["function_call_expression", "member_call_expression"],
            heritage_kinds: &["base_clause", "class_interface_clause"],
        },
    },
    LangEntry {
        name: "ruby",
        extensions: &["rb"],
        grammar: lang_ruby,
        spec: LangSpec {
            function_kinds: &["method", "singleton_method"],
            type_kinds: &["class", "module"],
            import_kinds: &["call"],  // require/require_relative are calls
            call_kinds: &["call", "method_call"],
            heritage_kinds: &["superclass"],
        },
    },
    LangEntry {
        name: "kotlin",
        extensions: &["kt", "kts"],
        grammar: lang_kotlin,
        spec: LangSpec {
            function_kinds: &["function_declaration"],
            type_kinds: &["class_declaration", "interface_declaration", "object_declaration", "enum_declaration"],
            import_kinds: &["import_header"],
            call_kinds: &["call_expression"],
            heritage_kinds: &["delegation_specifiers"],
        },
    },
    LangEntry {
        name: "swift",
        extensions: &["swift"],
        grammar: lang_swift,
        spec: LangSpec {
            function_kinds: &["function_declaration"],
            type_kinds: &["class_declaration", "struct_declaration", "protocol_declaration", "enum_declaration"],
            import_kinds: &["import_declaration"],
            call_kinds: &["call_expression"],
            heritage_kinds: &["inheritance_specifier"],
        },
    },
    LangEntry {
        name: "bash",
        extensions: &["sh", "bash", "zsh"],
        grammar: lang_bash,
        spec: LangSpec {
            function_kinds: &["function_definition"],
            type_kinds: &[],
            import_kinds: &[],
            call_kinds: &["command_name"],
            heritage_kinds: &[],
        },
    },
    LangEntry {
        name: "scala",
        extensions: &["scala", "sc"],
        grammar: lang_scala,
        spec: LangSpec {
            function_kinds: &["function_definition", "val_definition"],
            type_kinds: &["class_definition", "trait_definition", "object_definition"],
            import_kinds: &["import_declaration"],
            call_kinds: &["call_expression"],
            heritage_kinds: &[], // reached via the `extend` field, special-cased in walk()
        },
    },
];

fn entry_for(language: &str) -> Option<&'static LangEntry> {
    LANGS.iter().find(|e| e.name == language)
}

pub fn language_for_extension(ext: &str) -> Option<&'static str> {
    LANGS.iter().find(|e| e.extensions.contains(&ext)).map(|e| e.name)
}

fn spec_for(language: &str) -> Option<&'static LangSpec> {
    entry_for(language).map(|e| &e.spec)
}

fn ts_language(language: &str) -> Option<Language> {
    entry_for(language).map(|e| (e.grammar)())
}

pub fn parse_file(path: &str, language: &str, source: &str) -> Result<ParsedFile> {
    let Some(ts_lang) = ts_language(language) else { bail!("unsupported language: {language}") };
    let Some(spec) = spec_for(language) else { bail!("unsupported language: {language}") };

    let mut parser = Parser::new();
    parser.set_language(&ts_lang).map_err(|e| anyhow::anyhow!("failed to load {language} grammar: {e}"))?;
    let tree = parser.parse(source, None).ok_or_else(|| anyhow::anyhow!("tree-sitter failed to parse {path}"))?;

    let mut out = ParsedFile { nodes: Vec::new(), calls: Vec::new(), tests: Vec::new(), inherits: Vec::new() };
    let mut fn_stack: Vec<String> = Vec::new();
    walk(tree.root_node(), source, path, language, spec, &mut out, &mut fn_stack);
    Ok(out)
}

fn text<'a>(node: Node, source: &'a str) -> &'a str {
    node.utf8_text(source.as_bytes()).unwrap_or("").trim()
}

fn node_id(path: &str, start_line: usize, name: &str) -> String {
    format!("{path}#{start_line}:{name}")
}

fn make_node(path: &str, kind: CgNodeKind, name: &str, n: Node) -> CgNode {
    let start_line = n.start_position().row + 1;
    let end_line = n.end_position().row + 1;
    CgNode { id: node_id(path, start_line, name), file_path: path.to_string(), kind, name: name.to_string(), start_line, end_line }
}

/// Extract the callee name from a call/macro-invocation node, handling both
/// bare-identifier calls (`foo()`) and member calls (`x.foo()`), whatever
/// the language's field names for those shapes happen to be.
fn callee_name(call: Node, source: &str) -> Option<String> {
    let func = call.child_by_field_name("function").or_else(|| call.child_by_field_name("macro"))?;
    match func.kind() {
        "identifier" | "type_identifier" => Some(text(func, source).to_string()),
        "field_expression" | "member_expression" | "attribute" | "selector_expression" => {
            let field = func
                .child_by_field_name("field")
                .or_else(|| func.child_by_field_name("property"))
                .or_else(|| func.child_by_field_name("attribute"))?;
            Some(text(field, source).to_string())
        }
        _ => None,
    }
}

/// Best-effort "what does this test target" guess from naming convention
/// alone (no language has reliable structural test-to-target linkage).
fn guess_test_target(language: &str, test_name: &str) -> Option<String> {
    match language {
        "python" => test_name.strip_prefix("test_").map(str::to_string),
        "go" => test_name.strip_prefix("Test").map(str::to_string),
        "java" | "kotlin" | "swift" => test_name.strip_prefix("test").map(str::to_string),
        "php" => test_name.strip_prefix("test").map(str::to_string),
        "ruby" => test_name.strip_prefix("test_").map(str::to_string),
        _ => None,
    }
}

fn is_csharp_test_fn(n: Node, source: &str) -> bool {
    // C# tests have [Test] or [Fact] or [Theory] attribute on the previous sibling
    let mut cursor = n.walk();
    for child in n.parent().map(|p| p.children(&mut cursor).collect::<Vec<_>>()).unwrap_or_default() {
        if child.id() == n.id() { break; }
        if child.kind() == "attribute_list" && text(child, source).contains("Test") {
            return true;
        }
    }
    false
}

fn is_rust_test_fn(n: Node, source: &str) -> bool {
    n.prev_sibling().is_some_and(|sib| sib.kind() == "attribute_item" && text(sib, source).contains("test"))
}

/// Leaf token kinds that hold a type/superclass/interface name across the
/// grammars in `LANGS` — wider than `identifier`/`type_identifier` because
/// several languages name these tokens differently: Ruby's `constant`,
/// PHP's `name`/`qualified_name`/`relative_name`, C++'s `qualified_identifier`/
/// `template_type`, Swift's `user_type`, Scala's `stable_type_identifier`.
const HERITAGE_TOKEN_KINDS: &[&str] = &[
    "identifier",
    "type_identifier",
    "constant",
    "name",
    "qualified_name",
    "relative_name",
    "qualified_identifier",
    "template_type",
    "user_type",
    "stable_type_identifier",
];

/// All extends/implements/superclass names found anywhere under `n` (a
/// heritage clause node, or a single type-reference field for Rust's
/// `impl Trait for Type`). Stops descending once a heritage-token kind
/// matches — those are leaves in practice — so generic wrappers like
/// `generic_type`/`scoped_type_identifier` are transparently descended into.
fn heritage_names(n: Node, source: &str) -> Vec<String> {
    let mut out = Vec::new();
    collect_heritage_names(n, source, &mut out);
    out
}

fn collect_heritage_names(n: Node, source: &str, out: &mut Vec<String>) {
    if HERITAGE_TOKEN_KINDS.contains(&n.kind()) {
        if n.kind() == "user_type" {
            // Swift: prefer the inner type_identifier so `Foo<T>` reports "Foo", not the whole text.
            let mut cursor = n.walk();
            if let Some(inner) = n.children(&mut cursor).find(|c| c.kind() == "type_identifier") {
                out.push(text(inner, source).to_string());
                return;
            }
        }
        out.push(text(n, source).to_string());
        return;
    }
    let mut cursor = n.walk();
    for child in n.children(&mut cursor) {
        collect_heritage_names(child, source, out);
    }
}

fn walk(n: Node, source: &str, path: &str, language: &str, spec: &LangSpec, out: &mut ParsedFile, fn_stack: &mut Vec<String>) {
    let kind = n.kind();
    let mut pushed = false;

    // Rust has no classical inheritance — `impl Trait for Type` is the
    // closest analogue, and doesn't fit the type_kinds/heritage_kinds shape
    // below (the trait impl isn't itself a Type node), so it's handled here
    // independently of the function/type/import/call dispatch chain.
    if language == "rust" && kind == "impl_item" {
        if let (Some(ty), Some(tr)) = (n.child_by_field_name("type"), n.child_by_field_name("trait")) {
            if let (Some(sub), Some(sup)) = (heritage_names(ty, source).into_iter().next(), heritage_names(tr, source).into_iter().next())
            {
                out.inherits.push((sub, sup));
            }
        }
    }

    if spec.function_kinds.contains(&kind) {
        if let Some(name_node) = n.child_by_field_name("name") {
            let name = text(name_node, source).to_string();
            let is_method = n
                .parent()
                .map(|p| {
                    let pk = p.kind();
                    pk == "declaration_list" || pk == "class_body" || pk == "class" || pk == "interface_body" || pk == "struct_body"
                        || kind == "method_definition" || kind == "method_declaration" || kind == "constructor_declaration"
                        || kind == "singleton_method"
                })
                .unwrap_or(false);
            let node_kind = if is_method { CgNodeKind::Method } else { CgNodeKind::Function };
            let cg = make_node(path, node_kind, &name, n);

            let is_test = match language {
                "rust" => is_rust_test_fn(n, source),
                "python" | "go" | "java" | "kotlin" | "swift" | "php" | "ruby" => guess_test_target(language, &name).is_some(),
                "csharp" => is_csharp_test_fn(n, source),
                _ => false,
            };
            if is_test {
                let test_node = make_node(path, CgNodeKind::Test, &name, n);
                out.tests.push((test_node.id.clone(), guess_test_target(language, &name)));
                out.nodes.push(test_node);
            } else {
                out.nodes.push(cg.clone());
            }

            fn_stack.push(cg.id);
            pushed = true;
        }
    } else if spec.type_kinds.contains(&kind) {
        // Go's type_spec covers aliases/interfaces too; only struct types count as a "type" node here.
        let qualifies = kind != "type_spec" || n.child_by_field_name("type").is_some_and(|t| t.kind() == "struct_type");
        if qualifies && let Some(name_node) = n.child_by_field_name("name") {
            let type_name = text(name_node, source).to_string();
            out.nodes.push(make_node(path, CgNodeKind::Type, &type_name, n));

            let mut cursor = n.walk();
            for child in n.children(&mut cursor) {
                if spec.heritage_kinds.contains(&child.kind()) {
                    out.inherits.extend(heritage_names(child, source).into_iter().map(|sup| (type_name.clone(), sup)));
                }
            }
            // python/scala reach their heritage clause via a named field rather than a
            // child-kind lookup, since their grammars don't wrap it in its own node kind.
            let field_heritage = match language {
                "python" => n.child_by_field_name("superclasses"),
                "scala" => n.child_by_field_name("extend"),
                _ => None,
            };
            if let Some(field) = field_heritage {
                out.inherits.extend(heritage_names(field, source).into_iter().map(|sup| (type_name.clone(), sup)));
            }
        }
    } else if spec.import_kinds.contains(&kind) {
        let raw = text(n, source);
        if !raw.is_empty() {
            out.nodes.push(make_node(path, CgNodeKind::Import, raw, n));
        }
    } else if spec.call_kinds.contains(&kind) && let Some(callee) = callee_name(n, source) {
        out.calls.push((fn_stack.last().cloned(), callee));
    }

    for i in 0..n.child_count() {
        if let Some(child) = n.child(i) {
            walk(child, source, path, language, spec, out, fn_stack);
        }
    }

    if pushed {
        fn_stack.pop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(nodes: &[CgNode], kind: CgNodeKind) -> Vec<&str> {
        nodes.iter().filter(|n| n.kind == kind).map(|n| n.name.as_str()).collect()
    }

    #[test]
    fn parses_rust_functions_calls_and_test() {
        let src = r#"
fn helper() -> i32 { 42 }

fn main() {
    let x = helper();
    println!("{}", x);
}

#[test]
fn test_helper() {
    assert_eq!(helper(), 42);
}
"#;
        let parsed = parse_file("fixture.rs", "rust", src).unwrap();
        assert_eq!(names(&parsed.nodes, CgNodeKind::Function), vec!["helper", "main"]);
        assert_eq!(names(&parsed.nodes, CgNodeKind::Test), vec!["test_helper"]);
        assert!(parsed.calls.iter().any(|(caller, callee)| callee == "helper" && caller.as_deref() == Some("fixture.rs#4:main")));
    }

    #[test]
    fn parses_rust_struct_and_use() {
        let src = "use std::collections::HashMap;\n\nstruct Config { name: String }\n";
        let parsed = parse_file("fixture.rs", "rust", src).unwrap();
        assert_eq!(names(&parsed.nodes, CgNodeKind::Type), vec!["Config"]);
        assert_eq!(parsed.nodes.iter().filter(|n| n.kind == CgNodeKind::Import).count(), 1);
    }

    #[test]
    fn parses_rust_method_inside_impl() {
        let src = "struct Foo;\nimpl Foo {\n    fn bar(&self) -> i32 { 1 }\n}\n";
        let parsed = parse_file("fixture.rs", "rust", src).unwrap();
        assert_eq!(names(&parsed.nodes, CgNodeKind::Method), vec!["bar"]);
    }

    #[test]
    fn parses_python_function_and_test() {
        let src = "def add(a, b):\n    return a + b\n\ndef test_add():\n    assert add(1, 2) == 3\n";
        let parsed = parse_file("fixture.py", "python", src).unwrap();
        assert_eq!(names(&parsed.nodes, CgNodeKind::Function), vec!["add"]);
        assert_eq!(names(&parsed.nodes, CgNodeKind::Test), vec!["test_add"]);
        assert_eq!(parsed.tests[0].1.as_deref(), Some("add"));
        assert!(parsed.calls.iter().any(|(_, callee)| callee == "add"));
    }

    #[test]
    fn parses_python_class_and_import() {
        let src = "import os\nfrom collections import OrderedDict\n\nclass Widget:\n    pass\n";
        let parsed = parse_file("fixture.py", "python", src).unwrap();
        assert_eq!(names(&parsed.nodes, CgNodeKind::Type), vec!["Widget"]);
        assert_eq!(parsed.nodes.iter().filter(|n| n.kind == CgNodeKind::Import).count(), 2);
    }

    #[test]
    fn parses_typescript_function_class_and_call() {
        let src = "function double(x: number): number {\n  return helper(x);\n}\n\nclass Box {}\n";
        let parsed = parse_file("fixture.ts", "typescript", src).unwrap();
        assert_eq!(names(&parsed.nodes, CgNodeKind::Function), vec!["double"]);
        assert_eq!(names(&parsed.nodes, CgNodeKind::Type), vec!["Box"]);
        assert!(parsed.calls.iter().any(|(_, callee)| callee == "helper"));
    }

    #[test]
    fn parses_javascript_method_and_member_call() {
        let src = "class Greeter {\n  greet() {\n    return this.helper();\n  }\n}\n";
        let parsed = parse_file("fixture.js", "javascript", src).unwrap();
        assert_eq!(names(&parsed.nodes, CgNodeKind::Method), vec!["greet"]);
        assert!(parsed.calls.iter().any(|(_, callee)| callee == "helper"));
    }

    #[test]
    fn parses_go_function_struct_and_test() {
        let src = "package main\n\ntype Server struct {\n    Port int\n}\n\nfunc Add(a, b int) int {\n    return a + b\n}\n\nfunc TestAdd(t *testing.T) {\n    Add(1, 2)\n}\n";
        let parsed = parse_file("fixture.go", "go", src).unwrap();
        assert_eq!(names(&parsed.nodes, CgNodeKind::Type), vec!["Server"]);
        assert_eq!(names(&parsed.nodes, CgNodeKind::Function), vec!["Add"]);
        assert_eq!(names(&parsed.nodes, CgNodeKind::Test), vec!["TestAdd"]);
        assert_eq!(parsed.tests[0].1.as_deref(), Some("Add"));
    }

    #[test]
    fn unsupported_language_errors() {
        assert!(parse_file("x.lua", "lua", "print('hi')").is_err());
    }

    #[test]
    fn parses_rust_trait_impl_as_inherits() {
        let src = "trait Greet {}\nstruct Foo;\nimpl Greet for Foo {}\n";
        let parsed = parse_file("fixture.rs", "rust", src).unwrap();
        assert_eq!(parsed.inherits, vec![("Foo".to_string(), "Greet".to_string())]);
    }

    #[test]
    fn parses_typescript_extends_and_implements() {
        let src = "interface Shape {}\nclass Box implements Shape {}\nclass Square extends Box {}\n";
        let parsed = parse_file("fixture.ts", "typescript", src).unwrap();
        assert!(parsed.inherits.contains(&("Box".to_string(), "Shape".to_string())));
        assert!(parsed.inherits.contains(&("Square".to_string(), "Box".to_string())));
    }

    #[test]
    fn parses_python_superclasses() {
        let src = "class Animal:\n    pass\n\nclass Dog(Animal):\n    pass\n";
        let parsed = parse_file("fixture.py", "python", src).unwrap();
        assert_eq!(parsed.inherits, vec![("Dog".to_string(), "Animal".to_string())]);
    }

    #[test]
    fn parses_java_superclass_and_interfaces() {
        let src = "interface Flyable {}\ninterface Swimmable {}\nclass Duck extends Animal implements Flyable, Swimmable {}\n";
        let parsed = parse_file("fixture.java", "java", src).unwrap();
        assert!(parsed.inherits.contains(&("Duck".to_string(), "Animal".to_string())));
        assert!(parsed.inherits.contains(&("Duck".to_string(), "Flyable".to_string())));
        assert!(parsed.inherits.contains(&("Duck".to_string(), "Swimmable".to_string())));
    }

    #[test]
    fn parses_ruby_superclass() {
        let src = "class Animal\nend\n\nclass Dog < Animal\nend\n";
        let parsed = parse_file("fixture.rb", "ruby", src).unwrap();
        assert_eq!(parsed.inherits, vec![("Dog".to_string(), "Animal".to_string())]);
    }

    #[test]
    fn parses_cpp_base_class_clause() {
        let src = "class Animal {};\nclass Dog : public Animal {};\n";
        let parsed = parse_file("fixture.cpp", "cpp", src).unwrap();
        assert_eq!(parsed.inherits, vec![("Dog".to_string(), "Animal".to_string())]);
    }

    #[test]
    fn parses_swift_inheritance_specifier() {
        let src = "protocol Greetable {}\nclass Dog: Greetable {}\n";
        let parsed = parse_file("fixture.swift", "swift", src).unwrap();
        assert_eq!(parsed.inherits, vec![("Dog".to_string(), "Greetable".to_string())]);
    }

    #[test]
    fn parses_php_base_and_interface_clauses() {
        let src = "<?php\ninterface Flyable {}\nclass Animal {}\nclass Duck extends Animal implements Flyable {}\n";
        let parsed = parse_file("fixture.php", "php", src).unwrap();
        assert!(parsed.inherits.contains(&("Duck".to_string(), "Animal".to_string())));
        assert!(parsed.inherits.contains(&("Duck".to_string(), "Flyable".to_string())));
    }

    #[test]
    fn language_for_extension_covers_all_fifteen() {
        assert_eq!(language_for_extension("rs"), Some("rust"));
        assert_eq!(language_for_extension("ts"), Some("typescript"));
        assert_eq!(language_for_extension("js"), Some("javascript"));
        assert_eq!(language_for_extension("py"), Some("python"));
        assert_eq!(language_for_extension("go"), Some("go"));
        assert_eq!(language_for_extension("c"), Some("c"));
        assert_eq!(language_for_extension("h"), Some("c"));
        assert_eq!(language_for_extension("cpp"), Some("cpp"));
        assert_eq!(language_for_extension("java"), Some("java"));
        assert_eq!(language_for_extension("cs"), Some("csharp"));
        assert_eq!(language_for_extension("php"), Some("php"));
        assert_eq!(language_for_extension("rb"), Some("ruby"));
        assert_eq!(language_for_extension("kt"), Some("kotlin"));
        assert_eq!(language_for_extension("swift"), Some("swift"));
        assert_eq!(language_for_extension("sh"), Some("bash"));
        assert_eq!(language_for_extension("scala"), Some("scala"));
        assert_eq!(language_for_extension("xyz"), None);
    }
}
