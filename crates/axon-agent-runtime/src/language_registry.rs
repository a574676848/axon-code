//! 文件语言识别的唯一事实源，供 LSP 与结构化编辑共同使用。

use std::path::Path;

const LSP_PLAINTEXT_LANGUAGE_ID: &str = "plaintext";

/// Axon 支持的 tree-sitter grammar。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SyntaxGrammar {
    Astro,
    Bash,
    C,
    Cmake,
    Cpp,
    CSharp,
    Dart,
    Clojure,
    Css,
    Diff,
    Dockerfile,
    EmacsLisp,
    Elixir,
    Erlang,
    Fortran,
    Go,
    Graphql,
    Haskell,
    Hcl,
    Html,
    Ini,
    Java,
    JavaScript,
    Json,
    Just,
    Julia,
    Kotlin,
    Lua,
    Make,
    Markdown,
    Nix,
    ObjC,
    Ocaml,
    Odin,
    Php,
    Powershell,
    Proto,
    Python,
    R,
    Regex,
    Ruby,
    Rust,
    Scala,
    Solidity,
    Sql,
    Starlark,
    Svelte,
    Swift,
    Toml,
    Tlaplus,
    Tsx,
    TypeScript,
    Verilog,
    Vue,
    Xml,
    Yaml,
    Zig,
}

impl SyntaxGrammar {
    const ALL: &'static [Self] = &[
        Self::Astro,
        Self::Bash,
        Self::C,
        Self::Cmake,
        Self::Cpp,
        Self::CSharp,
        Self::Dart,
        Self::Clojure,
        Self::Css,
        Self::Diff,
        Self::Dockerfile,
        Self::EmacsLisp,
        Self::Elixir,
        Self::Erlang,
        Self::Fortran,
        Self::Go,
        Self::Graphql,
        Self::Haskell,
        Self::Hcl,
        Self::Html,
        Self::Ini,
        Self::Java,
        Self::JavaScript,
        Self::Json,
        Self::Just,
        Self::Julia,
        Self::Kotlin,
        Self::Lua,
        Self::Make,
        Self::Markdown,
        Self::Nix,
        Self::ObjC,
        Self::Ocaml,
        Self::Odin,
        Self::Php,
        Self::Powershell,
        Self::Proto,
        Self::Python,
        Self::R,
        Self::Regex,
        Self::Ruby,
        Self::Rust,
        Self::Scala,
        Self::Solidity,
        Self::Sql,
        Self::Starlark,
        Self::Svelte,
        Self::Swift,
        Self::Toml,
        Self::Tlaplus,
        Self::Tsx,
        Self::TypeScript,
        Self::Verilog,
        Self::Vue,
        Self::Xml,
        Self::Yaml,
        Self::Zig,
    ];

    pub(crate) const fn canonical_name(self) -> &'static str {
        match self {
            Self::Astro => "astro",
            Self::Bash => "bash",
            Self::C => "c",
            Self::Cmake => "cmake",
            Self::Cpp => "cpp",
            Self::CSharp => "csharp",
            Self::Dart => "dart",
            Self::Clojure => "clojure",
            Self::Css => "css",
            Self::Diff => "diff",
            Self::Dockerfile => "dockerfile",
            Self::EmacsLisp => "emacs-lisp",
            Self::Elixir => "elixir",
            Self::Erlang => "erlang",
            Self::Fortran => "fortran",
            Self::Go => "go",
            Self::Graphql => "graphql",
            Self::Haskell => "haskell",
            Self::Hcl => "hcl",
            Self::Html => "html",
            Self::Ini => "ini",
            Self::Java => "java",
            Self::JavaScript => "javascript",
            Self::Json => "json",
            Self::Just => "just",
            Self::Julia => "julia",
            Self::Kotlin => "kotlin",
            Self::Lua => "lua",
            Self::Make => "make",
            Self::Markdown => "markdown",
            Self::Nix => "nix",
            Self::ObjC => "objc",
            Self::Ocaml => "ocaml",
            Self::Odin => "odin",
            Self::Php => "php",
            Self::Powershell => "powershell",
            Self::Proto => "protobuf",
            Self::Python => "python",
            Self::R => "r",
            Self::Regex => "regex",
            Self::Ruby => "ruby",
            Self::Rust => "rust",
            Self::Scala => "scala",
            Self::Solidity => "solidity",
            Self::Sql => "sql",
            Self::Starlark => "starlark",
            Self::Svelte => "svelte",
            Self::Swift => "swift",
            Self::Toml => "toml",
            Self::Tlaplus => "tlaplus",
            Self::Tsx => "tsx",
            Self::TypeScript => "typescript",
            Self::Verilog => "verilog",
            Self::Vue => "vue",
            Self::Xml => "xml",
            Self::Yaml => "yaml",
            Self::Zig => "zig",
        }
    }

    pub(crate) fn tree_sitter_language(self) -> tree_sitter::Language {
        match self {
            Self::Astro => tree_sitter_astro::LANGUAGE.into(),
            Self::Bash => tree_sitter_bash::LANGUAGE.into(),
            Self::C => tree_sitter_c::LANGUAGE.into(),
            Self::Cmake => tree_sitter_cmake::LANGUAGE.into(),
            Self::Cpp => tree_sitter_cpp::LANGUAGE.into(),
            Self::CSharp => tree_sitter_c_sharp::LANGUAGE.into(),
            Self::Dart => tree_sitter_dart::LANGUAGE.into(),
            Self::Clojure => tree_sitter_clojure::LANGUAGE.into(),
            Self::Css => tree_sitter_css::LANGUAGE.into(),
            Self::Diff => tree_sitter_diff::LANGUAGE.into(),
            Self::Dockerfile => tree_sitter_dockerfile::language(),
            Self::EmacsLisp => tree_sitter_elisp::LANGUAGE.into(),
            Self::Elixir => tree_sitter_elixir::LANGUAGE.into(),
            Self::Erlang => tree_sitter_erlang::LANGUAGE.into(),
            Self::Fortran => tree_sitter_fortran::LANGUAGE.into(),
            Self::Go => tree_sitter_go::LANGUAGE.into(),
            Self::Graphql => tree_sitter_graphql::LANGUAGE.into(),
            Self::Haskell => tree_sitter_haskell::LANGUAGE.into(),
            Self::Hcl => tree_sitter_hcl::LANGUAGE.into(),
            Self::Html => tree_sitter_html::LANGUAGE.into(),
            Self::Ini => tree_sitter_ini::LANGUAGE.into(),
            Self::Java => tree_sitter_java::LANGUAGE.into(),
            Self::JavaScript => tree_sitter_javascript::LANGUAGE.into(),
            Self::Json => tree_sitter_json::LANGUAGE.into(),
            Self::Just => tree_sitter_just::LANGUAGE.into(),
            Self::Julia => tree_sitter_julia::LANGUAGE.into(),
            Self::Kotlin => tree_sitter_kotlin::LANGUAGE.into(),
            Self::Lua => tree_sitter_lua::LANGUAGE.into(),
            Self::Make => tree_sitter_make::LANGUAGE.into(),
            Self::Markdown => tree_sitter_md::LANGUAGE.into(),
            Self::Nix => tree_sitter_nix::LANGUAGE.into(),
            Self::ObjC => tree_sitter_objc::LANGUAGE.into(),
            Self::Ocaml => tree_sitter_ocaml::LANGUAGE_OCAML.into(),
            Self::Odin => tree_sitter_odin::LANGUAGE.into(),
            Self::Php => tree_sitter_php::LANGUAGE_PHP_ONLY.into(),
            Self::Powershell => tree_sitter_powershell::LANGUAGE.into(),
            Self::Proto => tree_sitter_proto::LANGUAGE.into(),
            Self::Python => tree_sitter_python::LANGUAGE.into(),
            Self::R => tree_sitter_r::LANGUAGE.into(),
            Self::Regex => tree_sitter_regex::LANGUAGE.into(),
            Self::Ruby => tree_sitter_ruby::LANGUAGE.into(),
            Self::Rust => tree_sitter_rust::LANGUAGE.into(),
            Self::Scala => tree_sitter_scala::LANGUAGE.into(),
            Self::Solidity => tree_sitter_solidity::LANGUAGE.into(),
            Self::Sql => tree_sitter_sql::LANGUAGE.into(),
            Self::Starlark => tree_sitter_starlark::LANGUAGE.into(),
            Self::Svelte => tree_sitter_svelte::LANGUAGE.into(),
            Self::Swift => tree_sitter_swift::LANGUAGE.into(),
            Self::Toml => tree_sitter_toml_ng::LANGUAGE.into(),
            Self::Tlaplus => tree_sitter_tlaplus::LANGUAGE.into(),
            Self::Tsx => tree_sitter_typescript::LANGUAGE_TSX.into(),
            Self::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            Self::Verilog => tree_sitter_verilog::LANGUAGE.into(),
            Self::Vue => tree_sitter_vue::LANGUAGE.into(),
            Self::Xml => tree_sitter_xml::LANGUAGE_XML.into(),
            Self::Yaml => tree_sitter_yaml::LANGUAGE.into(),
            Self::Zig => tree_sitter_zig::LANGUAGE.into(),
        }
    }

    fn from_path(path: &Path) -> Option<Self> {
        let name = path.file_name()?.to_str()?;
        match name {
            "Makefile" | "makefile" | "GNUmakefile" => return Some(Self::Make),
            "Justfile" | "justfile" => return Some(Self::Just),
            "CMakeLists.txt" => return Some(Self::Cmake),
            ".emacs" => return Some(Self::EmacsLisp),
            _ => {}
        }
        if name == "Dockerfile"
            || name == "dockerfile"
            || name.starts_with("Dockerfile.")
            || name.starts_with("dockerfile.")
            || name == "Containerfile"
            || name == "containerfile"
        {
            return Some(Self::Dockerfile);
        }
        let stem = name.strip_prefix('.').unwrap_or(name);
        if matches!(
            stem,
            "zshrc"
                | "zshenv"
                | "zprofile"
                | "zlogin"
                | "zlogout"
                | "zsh_aliases"
                | "bashrc"
                | "bash_profile"
                | "bash_login"
                | "bash_logout"
                | "bash_aliases"
                | "profile"
                | "kshrc"
                | "mkshrc"
                | "shrc"
        ) {
            return Some(Self::Bash);
        }
        let extension = path.extension()?.to_str()?;
        Self::ALL
            .iter()
            .copied()
            .find(|grammar| grammar.extensions().contains(&extension))
    }

    const fn extensions(self) -> &'static [&'static str] {
        match self {
            Self::Astro => &["astro"],
            Self::Bash => &[
                "bash", "bats", "cgi", "command", "env", "fcgi", "ksh", "sh", "tmux", "tool", "zsh",
            ],
            Self::C => &["c", "h"],
            Self::Cmake => &["cmake"],
            Self::Cpp => &["cc", "hpp", "cpp", "c++", "hh", "cxx", "cu", "ino"],
            Self::CSharp => &["cs"],
            Self::Dart => &["dart"],
            Self::Clojure => &["clj", "cljs", "cljc", "edn"],
            Self::Css => &["css", "scss"],
            Self::Diff => &["diff", "patch"],
            Self::Dockerfile => &["dockerfile"],
            Self::EmacsLisp => &["el"],
            Self::Elixir => &["ex", "exs"],
            Self::Erlang => &["erl", "hrl"],
            Self::Fortran => &["f90", "F90", "f95", "F95", "f03", "F03", "f08", "F08"],
            Self::Go => &["go"],
            Self::Graphql => &["graphql", "gql"],
            Self::Haskell => &["hs"],
            Self::Hcl => &["hcl", "tf", "tfvars"],
            Self::Html => &["html", "htm", "xhtml"],
            Self::Ini => &["ini", "cfg", "conf", "properties"],
            Self::Java => &["java"],
            Self::JavaScript => &["cjs", "js", "mjs", "jsx"],
            Self::Json => &["json"],
            Self::Just => &[],
            Self::Julia => &["jl"],
            Self::Kotlin => &["kt", "ktm", "kts"],
            Self::Lua => &["lua"],
            Self::Make => &["mk", "mak"],
            Self::Markdown => &["md", "markdown", "mdx"],
            Self::Nix => &["nix"],
            Self::ObjC => &["m"],
            Self::Ocaml => &["ml"],
            Self::Odin => &["odin"],
            Self::Php => &["php"],
            Self::Powershell => &["ps1", "psm1"],
            Self::Proto => &["proto"],
            Self::Python => &["py", "py3", "pyi", "bzl"],
            Self::R => &["r"],
            Self::Regex => &[],
            Self::Ruby => &["rb", "rbw", "gemspec"],
            Self::Rust => &["rs"],
            Self::Scala => &["scala", "sc", "sbt"],
            Self::Solidity => &["sol"],
            Self::Sql => &["sql"],
            Self::Starlark => &["star", "bzl"],
            Self::Svelte => &["svelte"],
            Self::Swift => &["swift"],
            Self::Toml => &["toml"],
            Self::Tlaplus => &["tla"],
            Self::Tsx => &["tsx"],
            Self::TypeScript => &["ts", "cts", "mts"],
            Self::Verilog => &["v", "sv", "svh", "vh"],
            Self::Vue => &["vue"],
            Self::Xml => &["xml", "xsl", "xslt", "svg", "plist"],
            Self::Yaml => &["yaml", "yml"],
            Self::Zig => &["zig"],
        }
    }
}

/// 一次文件语言解析的稳定结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResolvedLanguage {
    lookup_key: String,
    lsp_language_id: &'static str,
    grammar: Option<SyntaxGrammar>,
}

impl ResolvedLanguage {
    pub(crate) fn lookup_key(&self) -> &str {
        &self.lookup_key
    }

    pub(crate) const fn lsp_language_id(&self) -> &'static str {
        self.lsp_language_id
    }

    pub(crate) const fn grammar(&self) -> Option<SyntaxGrammar> {
        self.grammar
    }
}

/// Axon 文件语言注册表。
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct LanguageRegistry;

impl LanguageRegistry {
    /// 同时解析 LSP 路由键、LSP languageId 与 tree-sitter grammar。
    pub(crate) fn resolve(path: &Path) -> Option<ResolvedLanguage> {
        let lookup_key = lookup_key(path)?;
        let grammar = SyntaxGrammar::from_path(path);
        Some(ResolvedLanguage {
            lsp_language_id: lsp_language_id(path, grammar),
            grammar,
            lookup_key,
        })
    }
}

fn lookup_key(path: &Path) -> Option<String> {
    path.extension()
        .and_then(|value| value.to_str())
        .map(|value| format!(".{}", value.to_ascii_lowercase()))
        .or_else(|| {
            path.file_name()
                .and_then(|value| value.to_str())
                .map(ToOwned::to_owned)
        })
}

fn lsp_language_id(path: &Path, grammar: Option<SyntaxGrammar>) -> &'static str {
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    match extension.as_str() {
        "bash" | "ksh" | "sh" | "zsh" => "shellscript",
        "csx" => "csharp",
        "fsi" | "fsx" | "fsscript" => "fsharp",
        "gleam" => "gleam",
        "jsonc" => "json",
        "jsx" => "javascriptreact",
        "latex" => "latex",
        "lhs" => "haskell",
        "rake" | "ru" => "ruby",
        "razor" | "cshtml" => "razor",
        "tf" => "terraform",
        "tfvars" => "terraform-vars",
        "tsx" => "typescriptreact",
        "typ" | "typc" => "typst",
        "zon" => "zig",
        _ => grammar.map_or_else(
            || lsp_only_language_id(extension.as_str()),
            grammar_lsp_language_id,
        ),
    }
}

const fn grammar_lsp_language_id(grammar: SyntaxGrammar) -> &'static str {
    match grammar {
        SyntaxGrammar::Bash => "shellscript",
        SyntaxGrammar::Make => "makefile",
        SyntaxGrammar::Tsx => "typescriptreact",
        _ => grammar.canonical_name(),
    }
}

fn lsp_only_language_id(extension: &str) -> &'static str {
    match extension {
        "bib" => "bibtex",
        "cs" => "csharp",
        "fs" => "fsharp",
        "mli" => "ocaml",
        "prisma" => "prisma",
        "tex" => "latex",
        _ => LSP_PLAINTEXT_LANGUAGE_ID,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_lsp_and_tree_sitter_language_from_one_registry() {
        let tsx = LanguageRegistry::resolve(Path::new("src/app.tsx")).unwrap();
        assert_eq!(tsx.lookup_key(), ".tsx");
        assert_eq!(tsx.lsp_language_id(), "typescriptreact");
        assert_eq!(
            tsx.grammar().map(SyntaxGrammar::canonical_name),
            Some("tsx")
        );

        let terraform = LanguageRegistry::resolve(Path::new("infra/main.tf")).unwrap();
        assert_eq!(terraform.lsp_language_id(), "terraform");
        assert_eq!(
            terraform.grammar().map(SyntaxGrammar::canonical_name),
            Some("hcl")
        );
    }

    #[test]
    fn preserves_lsp_only_languages_without_claiming_a_grammar() {
        let fsharp = LanguageRegistry::resolve(Path::new("src/App.fs")).unwrap();
        assert_eq!(fsharp.lsp_language_id(), "fsharp");
        assert_eq!(fsharp.grammar(), None);
    }

    #[test]
    fn recognizes_special_file_names() {
        let dockerfile = LanguageRegistry::resolve(Path::new("Dockerfile")).unwrap();
        assert_eq!(dockerfile.lookup_key(), "Dockerfile");
        assert_eq!(dockerfile.lsp_language_id(), "dockerfile");
        assert_eq!(
            dockerfile.grammar().map(SyntaxGrammar::canonical_name),
            Some("dockerfile")
        );

        let zshrc = LanguageRegistry::resolve(Path::new(".zshrc")).unwrap();
        assert_eq!(zshrc.lsp_language_id(), "shellscript");
        assert_eq!(zshrc.grammar(), Some(SyntaxGrammar::Bash));
    }
}
