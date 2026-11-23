//! Writes koan-kit's API reference from its sources: every item a page can
//! name through the crate's public paths, with its doc comment and its
//! signature, as `docs/reference/koan-kit-api.md`; and the guide and the
//! reference together as one plain file for a reader that takes a single
//! document, `site/public/koan-kit.txt`.
//!
//! The sources are read with `syn` rather than through rustdoc, whose JSON
//! output needs a nightly toolchain. An item is public when a chain of
//! public modules reaches it or a public module re-exports it with
//! `pub use`; it is listed at its shortest such path.
//!
//! `--check` writes nothing and fails when either file differs from what it
//! would write: CI runs it, so the reference cannot drift from the code.
//! `--missing` lists the public items that have no doc comment.
//!
//! It reads and writes the workspace at `--root DIR`, or the current
//! directory, rather than the one it was built from: the crew's worktrees
//! share a target directory, so a binary built in one is run from another.

use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use syn::{Attribute, Expr, ImplItem, Item, Lit, Meta, TraitItem, Type, UseTree, Visibility};

/// The modules left out: tooling, and GPUI re-exported whole.
const SKIPPED: &[&str] = &["storybook", "raw"];

struct Module {
    /// From the crate's root: `["components", "button"]`.
    path: Vec<String>,
    /// Declared `pub` in its parent.
    public: bool,
    /// The file it is written in, from the crate's root.
    file: String,
    doc: String,
    items: Vec<Item>,
    /// Its `pub use` trees.
    uses: Vec<UseTree>,
}

/// One public item, at the path it is listed under.
struct Entry {
    module: usize,
    /// The module it is written in.
    source: usize,
    name: String,
    item: Item,
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let check = args.iter().any(|a| a == "--check");
    let missing = args.iter().any(|a| a == "--missing");
    let root = match args.iter().position(|a| a == "--root") {
        Some(i) => match args.get(i + 1) {
            Some(dir) => PathBuf::from(dir),
            None => {
                eprintln!("kit-reference: --root needs a directory");
                return ExitCode::FAILURE;
            }
        },
        None => PathBuf::from("."),
    };
    if !root.join("crates/koan-kit/src/lib.rs").is_file() {
        eprintln!(
            "kit-reference: {} is not the kōan workspace; run it from there or pass --root",
            root.display()
        );
        return ExitCode::FAILURE;
    }
    let src = root.join("crates/koan-kit/src");

    let mut modules = Vec::new();
    if let Err(e) = load(&src, &src.join("lib.rs"), Vec::new(), true, &mut modules) {
        eprintln!("kit-reference: {e}");
        return ExitCode::FAILURE;
    }
    let reference = Reference::new(&modules);
    let api = reference.markdown();
    let guide = match std::fs::read_to_string(root.join("docs/guide/koan-kit.md")) {
        Ok(text) => examples(&root, &text),
        Err(e) => {
            eprintln!("kit-reference: docs/guide/koan-kit.md: {e}");
            return ExitCode::FAILURE;
        }
    };
    let whole = whole(&guide, &api);

    if missing {
        for name in reference.undocumented() {
            println!("{name}");
        }
        return ExitCode::SUCCESS;
    }
    let outputs = [
        (root.join("docs/guide/koan-kit.md"), guide),
        (root.join("docs/reference/koan-kit-api.md"), api),
        (root.join("site/public/koan-kit.txt"), whole),
    ];
    let mut stale = false;
    for (path, text) in outputs {
        let shown = path
            .strip_prefix(&root)
            .unwrap_or(&path)
            .display()
            .to_string();
        if check {
            if std::fs::read_to_string(&path).ok().as_deref() != Some(text.as_str()) {
                eprintln!("{shown} is out of date: run `just kit-reference`");
                stale = true;
            }
        } else if let Err(e) = std::fs::write(&path, text) {
            eprintln!("kit-reference: {shown}: {e}");
            return ExitCode::FAILURE;
        }
    }
    if stale {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

/// Reads `file` as the module at `path`, and the modules it declares.
fn load(
    src: &Path,
    file: &Path,
    path: Vec<String>,
    public: bool,
    out: &mut Vec<Module>,
) -> Result<(), String> {
    let text = std::fs::read_to_string(file).map_err(|e| format!("{}: {e}", file.display()))?;
    let parsed = syn::parse_file(&text).map_err(|e| format!("{}: {e}", file.display()))?;
    // A child of `lib.rs` or `mod.rs` sits beside it; of `name.rs`, in `name/`.
    let dir = match file.file_stem().and_then(|s| s.to_str()) {
        Some("lib" | "mod") => file.parent().map(Path::to_path_buf),
        Some(stem) => file.parent().map(|p| p.join(stem)),
        None => None,
    }
    .unwrap_or_default();
    let shown = file.strip_prefix(src).unwrap_or(file).display().to_string();
    add(
        src,
        &dir,
        &shown,
        path,
        public,
        doc_of(&parsed.attrs),
        parsed.items,
        out,
    )
}

#[allow(clippy::too_many_arguments)]
fn add(
    src: &Path,
    dir: &Path,
    file: &str,
    path: Vec<String>,
    public: bool,
    doc: String,
    items: Vec<Item>,
    out: &mut Vec<Module>,
) -> Result<(), String> {
    let mut kept = Vec::new();
    let mut uses = Vec::new();
    let mut children = Vec::new();
    for item in items {
        if hidden(attrs(&item)) {
            continue;
        }
        match item {
            Item::Mod(m) => {
                let name = m.ident.to_string();
                if SKIPPED.contains(&name.as_str()) {
                    continue;
                }
                children.push((name, is_pub(&m.vis), m.content.map(|(_, i)| i), m.attrs));
            }
            Item::Use(u) if is_pub(&u.vis) => uses.push(u.tree),
            item => kept.push(item),
        }
    }
    out.push(Module {
        path: path.clone(),
        public,
        file: file.to_owned(),
        doc,
        items: kept,
        uses,
    });
    for (name, public, content, attrs) in children {
        let mut child = path.clone();
        child.push(name.clone());
        match content {
            Some(items) => add(
                src,
                &dir.join(&name),
                file,
                child,
                public,
                doc_of(&attrs),
                items,
                out,
            )?,
            None => {
                let flat = dir.join(format!("{name}.rs"));
                let file = if flat.exists() {
                    flat
                } else {
                    dir.join(&name).join("mod.rs")
                };
                load(src, &file, child, public, out)?;
                // The doc written on the declaration comes before the file's.
                let declared = doc_of(&attrs);
                if !declared.is_empty() {
                    let at = out
                        .iter()
                        .rposition(|m| m.path.last() == Some(&name))
                        .unwrap_or(0);
                    let module = &mut out[at];
                    module.doc = join(&declared, &module.doc);
                }
            }
        }
    }
    Ok(())
}

struct Reference<'a> {
    modules: &'a [Module],
    entries: Vec<Entry>,
    /// The items each module's `pub use` takes from outside the crate, by
    /// the path it lists them under: name, then where they come from.
    external: BTreeMap<Vec<String>, Vec<(String, String)>>,
    /// Methods and trait implementations, by the type's name, with the
    /// module each is written in.
    impls: HashMap<String, Vec<(usize, &'a syn::ItemImpl)>>,
    /// How many modules define a type of each name.
    defined: HashMap<String, usize>,
}

impl<'a> Reference<'a> {
    fn new(modules: &'a [Module]) -> Self {
        let by_path: HashMap<&[String], usize> = modules
            .iter()
            .enumerate()
            .map(|(i, m)| (m.path.as_slice(), i))
            .collect();
        // A module's own path is public when every module on it is.
        let open = |i: usize| -> bool {
            (0..=modules[i].path.len()).all(|n| {
                by_path
                    .get(&modules[i].path[..n])
                    .is_some_and(|&j| modules[j].public)
            })
        };
        // Where each item can be named: (module, name) to listed paths.
        let mut places: HashMap<(usize, String), Vec<Vec<String>>> = HashMap::new();
        for (i, module) in modules.iter().enumerate() {
            if !open(i) {
                continue;
            }
            for item in &module.items {
                if let Some(name) = name_of(item).filter(|_| public(item)) {
                    let mut at = module.path.clone();
                    at.push(name.clone());
                    places.entry((i, name)).or_default().push(at);
                }
            }
        }
        let mut external: BTreeMap<Vec<String>, Vec<(String, String)>> = BTreeMap::new();
        for (i, module) in modules.iter().enumerate() {
            if !open(i) {
                continue;
            }
            for tree in &module.uses {
                let mut flat = Vec::new();
                flatten(tree, Vec::new(), &mut flat);
                for (segments, alias) in flat {
                    match resolve(&module.path, &segments, &by_path) {
                        Some((target, Some(name))) => {
                            let shown = alias.clone().unwrap_or_else(|| name.clone());
                            if modules[target].items.iter().any(|item| {
                                public(item) && name_of(item).as_deref() == Some(name.as_str())
                            }) {
                                let mut at = module.path.clone();
                                at.push(shown);
                                places.entry((target, name)).or_default().push(at);
                            }
                        }
                        Some((target, None)) => {
                            for item in &modules[target].items {
                                if let Some(name) = name_of(item).filter(|_| public(item)) {
                                    let mut at = module.path.clone();
                                    at.push(name.clone());
                                    places.entry((target, name)).or_default().push(at);
                                }
                            }
                        }
                        None => {
                            let from = segments.join("::");
                            let shown = alias
                                .unwrap_or_else(|| segments.last().cloned().unwrap_or_default());
                            // `gpui_platform as platform` names a crate.
                            let from = if segments.len() == 1 {
                                format!("{from}::{from}")
                            } else {
                                from
                            };
                            external
                                .entry(module.path.clone())
                                .or_default()
                                .push((shown, from));
                        }
                    }
                }
            }
        }
        let mut entries = Vec::new();
        for ((target, name), mut paths) in places {
            paths.sort_by(|a, b| a.len().cmp(&b.len()).then(a.cmp(b)));
            let at = paths.swap_remove(0);
            let listed = at[..at.len() - 1].to_vec();
            let Some(&module) = by_path.get(listed.as_slice()) else {
                continue;
            };
            let Some(item) = modules[target]
                .items
                .iter()
                .find(|item| name_of(item).as_deref() == Some(name.as_str()))
            else {
                continue;
            };
            entries.push((
                (
                    listed,
                    modules[target].file.clone(),
                    order(&modules[target], item),
                ),
                Entry {
                    module,
                    source: target,
                    name: at.last().cloned().unwrap_or(name),
                    item: item.clone(),
                },
            ));
        }
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        let mut impls: HashMap<String, Vec<(usize, &syn::ItemImpl)>> = HashMap::new();
        let mut defined: HashMap<String, usize> = HashMap::new();
        for (m, module) in modules.iter().enumerate() {
            for item in &module.items {
                match item {
                    Item::Impl(i) => {
                        if let Some(ty) = type_name(&i.self_ty) {
                            impls.entry(ty).or_default().push((m, i));
                        }
                    }
                    Item::Struct(_) | Item::Enum(_) => {
                        if let Some(name) = name_of(item) {
                            *defined.entry(name).or_default() += 1;
                        }
                    }
                    _ => {}
                }
            }
        }
        Self {
            modules,
            entries: entries.into_iter().map(|(_, e)| e).collect(),
            external,
            impls,
            defined,
        }
    }

    fn markdown(&self) -> String {
        let mut out = String::new();
        out.push_str(
            "# koan-kit API reference\n\n\
             <!-- Generated by `just kit-reference` from koan-kit's sources; edit the doc \
             comments, not this file. -->\n\n\
             Every item an app can name through koan-kit's public paths, with its doc \
             comment and signature, grouped by the module it is named from and then by \
             the file it is written in. The [guide](../guide/koan-kit.md) says how the \
             parts fit together. `koan_kit::raw`, GPUI re-exported whole, is left out: \
             an app reaches for it only where nothing here serves, and says why.\n",
        );
        // Every module that names something, in path order: its entries,
        // and what it re-exports from GPUI.
        let mut listed: BTreeMap<Vec<String>, Vec<&Entry>> = BTreeMap::new();
        for entry in &self.entries {
            listed
                .entry(self.modules[entry.module].path.clone())
                .or_default()
                .push(entry);
        }
        for path in self.external.keys() {
            listed.entry(path.clone()).or_default();
        }
        for (path, entries) in &listed {
            let _ = write!(out, "\n## `{}`\n", crate_path(path));
            if let Some(module) = self.modules.iter().find(|m| &m.path == path)
                && !module.doc.is_empty()
            {
                let _ = write!(out, "\n{}\n", demote(&module.doc));
            }
            if let Some(names) = self.external.get(path) {
                // By the module they come from: `gpui`, `gpui::prelude`.
                let mut by_source: BTreeMap<String, Vec<String>> = BTreeMap::new();
                for (name, from) in names {
                    let (module, item) = from.rsplit_once("::").unwrap_or(("", from));
                    let shown = match (item, name == item) {
                        ("*", _) => "everything".to_owned(),
                        (_, true) => format!("`{name}`"),
                        (_, false) => format!("`{item}` as `{name}`"),
                    };
                    by_source.entry(module.to_owned()).or_default().push(shown);
                }
                out.push('\n');
                for (module, names) in by_source {
                    let _ = writeln!(out, "Re-exported from `{module}`: {}.\n", names.join(", "));
                }
            }
            let mut file = None;
            for entry in entries {
                let source = self.source(entry);
                if file != Some(&source.file) && &source.path != path {
                    file = Some(&source.file);
                    let _ = write!(out, "\n### {}\n", source.path.join("::"));
                    if !source.doc.is_empty() {
                        let _ = write!(out, "\n{}\n", demote(&source.doc));
                    }
                }
                self.item(entry, &mut out);
            }
        }
        out
    }

    /// The impl blocks for an entry's type: any of that name, or where two
    /// modules define one, those written in the entry's own module.
    fn impls_of(&self, entry: &Entry) -> Vec<&'a syn::ItemImpl> {
        let Some(ty) = name_of(&entry.item) else {
            return Vec::new();
        };
        let shared = self.defined.get(&ty).is_some_and(|&n| n > 1);
        self.impls
            .get(&ty)
            .into_iter()
            .flatten()
            .filter(|(m, _)| !shared || *m == entry.source)
            .map(|(_, i)| *i)
            .collect()
    }

    /// The module an entry is written in.
    fn source(&self, entry: &Entry) -> &Module {
        &self.modules[entry.source]
    }

    fn item(&self, entry: &Entry, out: &mut String) {
        let (kind, doc) = (kind(&entry.item), doc_of(attrs(&entry.item)));
        let _ = write!(out, "\n#### `{}`\n\n*{kind}*", entry.name);
        if !doc.is_empty() {
            let _ = write!(out, " · {}", demote(&doc));
        }
        out.push('\n');
        let _ = write!(out, "\n```rust\n{}\n```\n", signature(&entry.item));
        if !matches!(entry.item, Item::Struct(_) | Item::Enum(_)) {
            return;
        }
        let mut traits = Vec::new();
        let mut methods = Vec::new();
        for i in self.impls_of(entry) {
            match &i.trait_ {
                Some((path, _)) => {
                    if let Some(last) = path.segments.last() {
                        traits.push(last.ident.to_string());
                    }
                }
                None => {
                    for item in &i.items {
                        if let ImplItem::Fn(f) = item
                            && is_pub(&f.vis)
                            && !hidden(&f.attrs)
                        {
                            methods.push(f);
                        }
                    }
                }
            }
        }
        for f in methods {
            let _ = write!(out, "\n- `{}`", one_line(&fn_signature(&f.vis, &f.sig)));
            let doc = doc_of(&f.attrs);
            if !doc.is_empty() {
                let _ = write!(out, "\n\n  {}", indent(&demote(&doc)));
            }
            out.push('\n');
        }
        traits.sort();
        traits.dedup();
        if !traits.is_empty() {
            let _ = write!(
                out,
                "\nImplements {}.\n",
                traits
                    .iter()
                    .map(|t| format!("`{t}`"))
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
    }

    fn undocumented(&self) -> Vec<String> {
        let mut out = Vec::new();
        for entry in &self.entries {
            let at = format!(
                "{}::{}",
                crate_path(&self.modules[entry.module].path),
                entry.name
            );
            if doc_of(attrs(&entry.item)).is_empty() {
                out.push(at.clone());
            }
            if matches!(entry.item, Item::Struct(_) | Item::Enum(_)) {
                for i in self.impls_of(entry) {
                    if i.trait_.is_some() {
                        continue;
                    }
                    for item in &i.items {
                        if let ImplItem::Fn(f) = item
                            && is_pub(&f.vis)
                            && !hidden(&f.attrs)
                            && doc_of(&f.attrs).is_empty()
                        {
                            out.push(format!("{at}::{}", f.sig.ident));
                        }
                    }
                }
            }
        }
        out
    }
}

/// The guide with each example it names (`<!-- example: PATH -->` over a
/// code block) as the file is now, so an example in the guide is one that
/// builds.
fn examples(root: &Path, guide: &str) -> String {
    let mut out = String::new();
    let mut lines = guide.lines();
    while let Some(line) = lines.next() {
        out.push_str(line);
        out.push('\n');
        let Some(path) = line
            .strip_prefix("<!-- example: ")
            .and_then(|l| l.strip_suffix(" -->"))
        else {
            continue;
        };
        let Some(fence) = lines.next() else { break };
        // The block as it stands is replaced whole.
        for line in lines.by_ref() {
            if line == "```" {
                break;
            }
        }
        let code = std::fs::read_to_string(root.join(path))
            .unwrap_or_else(|e| format!("// {path}: {e}\n"));
        out.push_str(fence);
        out.push('\n');
        out.push_str(&code);
        if !code.ends_with('\n') {
            out.push('\n');
        }
        out.push_str("```\n");
    }
    out
}

/// The guide and the reference as one file, its pictures as their words.
fn whole(guide: &str, api: &str) -> String {
    let mut out = String::from(
        "# koan-kit\n\n\
         This file is koan-kit's guide followed by its API reference, as one plain \
         document for a reader that takes a single file. The same text is published \
         at https://koan.rocks/docs/koan-kit/ and https://koan.rocks/docs/koan-kit-api/.\n\n",
    );
    for line in guide.lines() {
        match line.strip_prefix("![").and_then(|l| l.split_once("](")) {
            Some((alt, _)) => {
                let _ = writeln!(out, "(Picture: {alt}.)");
            }
            None => {
                out.push_str(line);
                out.push('\n');
            }
        }
    }
    out.push('\n');
    out.push_str(api);
    out
}

// Paths.

/// `pub use` trees as paths and the name each is given; a glob's path ends
/// at its module and has `*` for its name.
fn flatten(tree: &UseTree, prefix: Vec<String>, out: &mut Vec<(Vec<String>, Option<String>)>) {
    match tree {
        UseTree::Path(p) => {
            let mut prefix = prefix;
            prefix.push(p.ident.to_string());
            flatten(&p.tree, prefix, out);
        }
        UseTree::Name(n) => {
            let mut path = prefix;
            path.push(n.ident.to_string());
            out.push((path, None));
        }
        UseTree::Rename(r) => {
            let mut path = prefix;
            path.push(r.ident.to_string());
            out.push((path, Some(r.rename.to_string())));
        }
        UseTree::Glob(_) => {
            let mut path = prefix;
            path.push("*".into());
            out.push((path, None));
        }
        UseTree::Group(g) => {
            for tree in &g.items {
                flatten(tree, prefix.clone(), out);
            }
        }
    }
}

/// A `pub use` path as the module it names into and the item it names
/// there, `None` for a glob; `None` altogether for a path outside the crate.
fn resolve(
    from: &[String],
    segments: &[String],
    modules: &HashMap<&[String], usize>,
) -> Option<(usize, Option<String>)> {
    let (last, leading) = segments.split_last()?;
    let mut at = from.to_vec();
    for (n, segment) in leading.iter().enumerate() {
        match segment.as_str() {
            "crate" if n == 0 => at.clear(),
            "self" if n == 0 => {}
            "super" => {
                at.pop();
            }
            name => {
                at.push(name.to_owned());
                modules.get(at.as_slice())?;
            }
        }
    }
    let module = *modules.get(at.as_slice())?;
    if last == "*" {
        Some((module, None))
    } else {
        Some((module, Some(last.clone())))
    }
}

fn crate_path(path: &[String]) -> String {
    std::iter::once("koan_kit".to_owned())
        .chain(path.iter().cloned())
        .collect::<Vec<_>>()
        .join("::")
}

// Items.

fn attrs(item: &Item) -> &[Attribute] {
    match item {
        Item::Const(i) => &i.attrs,
        Item::Enum(i) => &i.attrs,
        Item::Fn(i) => &i.attrs,
        Item::Impl(i) => &i.attrs,
        Item::Macro(i) => &i.attrs,
        Item::Mod(i) => &i.attrs,
        Item::Static(i) => &i.attrs,
        Item::Struct(i) => &i.attrs,
        Item::Trait(i) => &i.attrs,
        Item::Type(i) => &i.attrs,
        Item::Use(i) => &i.attrs,
        _ => &[],
    }
}

fn name_of(item: &Item) -> Option<String> {
    Some(
        match item {
            Item::Const(i) => &i.ident,
            Item::Enum(i) => &i.ident,
            Item::Fn(i) => &i.sig.ident,
            Item::Static(i) => &i.ident,
            Item::Struct(i) => &i.ident,
            Item::Trait(i) => &i.ident,
            Item::Type(i) => &i.ident,
            _ => return None,
        }
        .to_string(),
    )
}

fn public(item: &Item) -> bool {
    let vis = match item {
        Item::Const(i) => &i.vis,
        Item::Enum(i) => &i.vis,
        Item::Fn(i) => &i.vis,
        Item::Static(i) => &i.vis,
        Item::Struct(i) => &i.vis,
        Item::Trait(i) => &i.vis,
        Item::Type(i) => &i.vis,
        _ => return false,
    };
    is_pub(vis) && !hidden(attrs(item))
}

fn is_pub(vis: &Visibility) -> bool {
    matches!(vis, Visibility::Public(_))
}

/// Left out of the reference: `#[doc(hidden)]`, and what is built only for
/// tests.
fn hidden(attrs: &[Attribute]) -> bool {
    attrs.iter().any(|a| {
        let text = match &a.meta {
            Meta::List(l) => l.tokens.to_string(),
            _ => return false,
        };
        (a.path().is_ident("doc") && text.contains("hidden"))
            || (a.path().is_ident("cfg") && text == "test")
    })
}

fn same(a: &Item, b: &Item) -> bool {
    name_of(a).is_some() && name_of(a) == name_of(b) && kind(a) == kind(b)
}

fn order(module: &Module, item: &Item) -> usize {
    module
        .items
        .iter()
        .position(|i| same(i, item))
        .unwrap_or(usize::MAX)
}

fn kind(item: &Item) -> &'static str {
    match item {
        Item::Const(_) => "constant",
        Item::Enum(_) => "enum",
        Item::Fn(_) => "function",
        Item::Static(_) => "static",
        Item::Struct(_) => "struct",
        Item::Trait(_) => "trait",
        Item::Type(_) => "type alias",
        _ => "item",
    }
}

fn type_name(ty: &Type) -> Option<String> {
    match ty {
        Type::Path(p) => p.path.segments.last().map(|s| s.ident.to_string()),
        _ => None,
    }
}

// Text.

fn doc_of(attrs: &[Attribute]) -> String {
    let lines: Vec<String> = attrs
        .iter()
        .filter(|a| a.path().is_ident("doc"))
        .filter_map(|a| match &a.meta {
            Meta::NameValue(nv) => match &nv.value {
                Expr::Lit(l) => match &l.lit {
                    Lit::Str(s) => Some(s.value()),
                    _ => None,
                },
                _ => None,
            },
            _ => None,
        })
        .map(|line| line.strip_prefix(' ').unwrap_or(&line).to_owned())
        .collect();
    lines.join("\n").trim().to_owned()
}

fn join(a: &str, b: &str) -> String {
    match (a.is_empty(), b.is_empty()) {
        (true, _) => b.to_owned(),
        (_, true) => a.to_owned(),
        _ => format!("{a}\n\n{b}"),
    }
}

/// Headings in a doc comment sit under the reference's own.
fn demote(doc: &str) -> String {
    doc.lines()
        .map(|line| {
            if line.starts_with('#') {
                format!("#####{line}")
            } else {
                line.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn indent(text: &str) -> String {
    text.replace('\n', "\n  ")
}

fn one_line(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .replace("( ", "(")
        .replace(" )", ")")
        .replace(", )", ")")
        .replace(",)", ")")
}

// Signatures.

fn signature(item: &Item) -> String {
    let shown = match item {
        Item::Struct(s) => {
            let private = s.fields.iter().any(|f| !is_pub(&f.vis));
            let mut s = s.clone();
            s.attrs.retain(|a| a.path().is_ident("derive"));
            if let syn::Fields::Named(fields) = &mut s.fields {
                fields.named = fields
                    .named
                    .iter()
                    .filter(|f| is_pub(&f.vis))
                    .cloned()
                    .map(|mut f| {
                        f.attrs.retain(|a| a.path().is_ident("doc"));
                        f
                    })
                    .collect();
            }
            if let syn::Fields::Unnamed(fields) = &mut s.fields {
                for f in fields.unnamed.iter_mut() {
                    f.attrs.clear();
                    if !is_pub(&f.vis) {
                        f.ty = syn::parse_quote!(_);
                    }
                }
            }
            let text = unparse(Item::Struct(s));
            // Fields the struct keeps to itself are left out, and said to be.
            return match (private, text.strip_suffix('}')) {
                (true, Some(open)) if text.ends_with("{}") => {
                    format!("{}{{ /* private fields */ }}", &open[..open.len() - 1])
                }
                (true, Some(open)) => format!("{open}    /* private fields */\n}}"),
                _ => text,
            };
        }
        Item::Enum(e) => {
            let mut e = e.clone();
            e.attrs.retain(|a| a.path().is_ident("derive"));
            for v in e.variants.iter_mut() {
                v.attrs.retain(|a| a.path().is_ident("doc"));
            }
            Item::Enum(e)
        }
        Item::Fn(f) => return fn_signature(&f.vis, &f.sig),
        Item::Trait(t) => {
            let mut t = t.clone();
            t.attrs.clear();
            for item in t.items.iter_mut() {
                if let TraitItem::Fn(f) = item {
                    f.default = None;
                    f.semi_token = Some(Default::default());
                    f.attrs.retain(|a| a.path().is_ident("doc"));
                }
            }
            Item::Trait(t)
        }
        Item::Const(c) => {
            let mut c = c.clone();
            c.attrs.clear();
            Item::Const(c)
        }
        Item::Static(s) => {
            let mut s = s.clone();
            s.attrs.clear();
            Item::Static(s)
        }
        Item::Type(t) => {
            let mut t = t.clone();
            t.attrs.clear();
            Item::Type(t)
        }
        other => other.clone(),
    };
    unparse(shown)
}

fn fn_signature(vis: &Visibility, sig: &syn::Signature) -> String {
    // How the function binds its arguments is not the caller's business.
    let mut sig = sig.clone();
    for input in sig.inputs.iter_mut() {
        match input {
            syn::FnArg::Receiver(r) => r.mutability = None,
            syn::FnArg::Typed(t) => {
                if let syn::Pat::Ident(i) = t.pat.as_mut() {
                    i.mutability = None;
                }
            }
        }
    }
    let f = syn::ItemFn {
        attrs: Vec::new(),
        vis: vis.clone(),
        modifiers: Default::default(),
        sig,
        block: Box::new(syn::Block {
            brace_token: Default::default(),
            stmts: Vec::new(),
        }),
    };
    let text = unparse(Item::Fn(f));
    text.trim_end()
        .strip_suffix("{}")
        .unwrap_or(&text)
        .trim_end()
        .to_owned()
}

fn unparse(item: Item) -> String {
    let file = syn::File {
        shebang: None,
        frontmatter: None,
        attrs: Vec::new(),
        items: vec![item],
    };
    prettyplease::unparse(&file).trim_end().to_owned()
}
