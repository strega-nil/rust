use crate::{Assist, AssistId, AssistCtx};

use hir::Resolver;
use hir::db::HirDatabase;
use ra_syntax::{SmolStr, SyntaxKind, TextRange, TextUnit, TreeArc};
use ra_syntax::ast::{self, AstNode, FnDef, ImplItem, ImplItemKind, NameOwner};
use ra_db::FilePosition;
use ra_fmt::{leading_indent, reindent};

use itertools::Itertools;

/// Given an `ast::ImplBlock`, resolves the target trait (the one being
/// implemented) to a `ast::TraitDef`.
pub(crate) fn resolve_target_trait_def(
    db: &impl HirDatabase,
    resolver: &Resolver,
    impl_block: &ast::ImplBlock,
) -> Option<TreeArc<ast::TraitDef>> {
    let ast_path = impl_block.target_trait().map(AstNode::syntax).and_then(ast::PathType::cast)?;
    let hir_path = ast_path.path().and_then(hir::Path::from_ast)?;

    match resolver.resolve_path(db, &hir_path).take_types() {
        Some(hir::Resolution::Def(hir::ModuleDef::Trait(def))) => Some(def.source(db).1),
        _ => None,
    }
}

pub(crate) fn build_func_body(def: &ast::FnDef) -> String {
    let mut buf = String::new();

    for child in def.syntax().children() {
        if child.kind() == SyntaxKind::SEMI {
            buf.push_str(" { unimplemented!() }")
        } else {
            child.text().push_to(&mut buf);
        }
    }

    buf.trim_end().to_string()
}

pub(crate) fn add_missing_impl_members(mut ctx: AssistCtx<impl HirDatabase>) -> Option<Assist> {
    let node = ctx.covering_node();
    let impl_node = node.ancestors().find_map(ast::ImplBlock::cast)?;
    let impl_item_list = impl_node.item_list()?;
    // Don't offer the assist when cursor is at the end, outside the block itself.
    if node.range().end() == impl_node.syntax().range().end() {
        return None;
    }

    let trait_def = {
        let db = ctx.db;
        // TODO: Can we get the position of cursor itself rather than supplied range?
        let range = ctx.frange;
        let position = FilePosition { file_id: range.file_id, offset: range.range.start() };
        let resolver = hir::source_binder::resolver_for_position(db, position);

        resolve_target_trait_def(db, &resolver, impl_node)?
    };

    let fn_def_opt = |kind| if let ImplItemKind::FnDef(def) = kind { Some(def) } else { None };
    let def_name = |def| -> Option<&SmolStr> { FnDef::name(def).map(ast::Name::text) };

    let trait_items = trait_def.syntax().descendants().find_map(ast::ItemList::cast)?.impl_items();
    let impl_items = impl_item_list.impl_items();

    let trait_fns = trait_items.map(ImplItem::kind).filter_map(fn_def_opt).collect::<Vec<_>>();
    let impl_fns = impl_items.map(ImplItem::kind).filter_map(fn_def_opt).collect::<Vec<_>>();

    let missing_fns: Vec<_> = trait_fns
        .into_iter()
        .filter(|t| impl_fns.iter().all(|i| def_name(i) != def_name(t)))
        .collect();
    if missing_fns.is_empty() {
        return None;
    }

    ctx.add_action(AssistId("add_impl_missing_members"), "add missing impl members", |edit| {
        let indent = {
            // FIXME: Find a way to get the indent already used in the file.
            // Now, we copy the indent of first item or indent with 4 spaces relative to impl block
            const DEFAULT_INDENT: &str = "    ";
            let first_item = impl_item_list.impl_items().next();
            let first_item_indent = first_item.and_then(|i| leading_indent(i.syntax()));
            let impl_block_indent = || leading_indent(impl_node.syntax()).unwrap_or_default();

            first_item_indent
                .map(ToOwned::to_owned)
                .unwrap_or_else(|| impl_block_indent().to_owned() + DEFAULT_INDENT)
        };

        let mut func_bodies = missing_fns.into_iter().map(build_func_body);
        let func_bodies = func_bodies.join("\n");
        let func_bodies = String::from("\n") + &func_bodies;
        let func_bodies = reindent(&func_bodies, &indent) + "\n";

        let changed_range = {
            let last_whitespace = impl_item_list.syntax().children();
            let last_whitespace = last_whitespace.filter_map(ast::Whitespace::cast).last();
            let last_whitespace = last_whitespace.map(|w| w.syntax());

            let cursor_range = TextRange::from_to(node.range().end(), node.range().end());

            last_whitespace.map(|x| x.range()).unwrap_or(cursor_range)
        };

        let replaced_text_range = TextUnit::of_str(&func_bodies);

        edit.replace(changed_range, func_bodies);
        edit.set_cursor(changed_range.start() + replaced_text_range - TextUnit::of_str("\n"));
    });

    ctx.build()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::helpers::{check_assist, check_assist_not_applicable};

    #[test]
    fn test_add_missing_impl_members() {
        check_assist(
            add_missing_impl_members,
            "
trait Foo {
    fn foo(&self);
    fn bar(&self);
    fn baz(&self);
}

struct S;

impl Foo for S {
    fn bar(&self) {}
    <|>
}",
            "
trait Foo {
    fn foo(&self);
    fn bar(&self);
    fn baz(&self);
}

struct S;

impl Foo for S {
    fn bar(&self) {}
    fn foo(&self) { unimplemented!() }
    fn baz(&self) { unimplemented!() }<|>
}",
        );
    }

    #[test]
    fn test_empty_impl_block() {
        check_assist(
            add_missing_impl_members,
            "
trait Foo { fn foo(&self); }
struct S;
impl Foo for S {<|>}",
            "
trait Foo { fn foo(&self); }
struct S;
impl Foo for S {
    fn foo(&self) { unimplemented!() }<|>
}",
        );
    }

    #[test]
    fn test_cursor_after_empty_impl_block() {
        check_assist_not_applicable(
            add_missing_impl_members,
            "
trait Foo { fn foo(&self); }
struct S;
impl Foo for S {}<|>",
        )
    }
}
