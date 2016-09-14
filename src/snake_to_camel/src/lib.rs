#![feature(plugin_registrar, rustc_private)]

extern crate itertools;
extern crate rustc;
extern crate rustc_plugin;
extern crate syntax;

use itertools::Itertools;
use syntax::ast::{self, Ident, TraitRef, Ty, TyKind};
use syntax::parse::{self, PResult, token};
use syntax::ptr::P;
use syntax::parse::parser::{Parser, PathStyle};
use syntax::tokenstream::TokenTree;
use syntax::ext::base::{DummyResult, ExtCtxt, MacEager, MacResult};
use syntax::ext::quote::rt::Span;
use syntax::util::small_vector::SmallVector;
use rustc_plugin::Registry;

fn snake_to_camel(cx: &mut ExtCtxt, sp: Span, tts: &[TokenTree]) -> Box<MacResult + 'static> {
    let mut parser = parse::new_parser_from_tts(cx.parse_sess(), cx.cfg(), tts.into());
    // The `expand_expr` method is called so that any macro calls in the
    // parsed expression are expanded.

    let mut item = match parser.parse_trait_item() {
        Ok(s) => s,
        Err(mut diagnostic) => {
            diagnostic.emit();
            return DummyResult::any(sp);
        }
    };

    if let Err(mut diagnostic) = parser.expect(&token::Eof) {
        diagnostic.emit();
        return DummyResult::any(sp);
    }

    convert(&mut item.ident);
    MacEager::trait_items(SmallVector::one(item))
}

fn impl_snake_to_camel(cx: &mut ExtCtxt, sp: Span, tts: &[TokenTree]) -> Box<MacResult + 'static> {
    let mut parser = parse::new_parser_from_tts(cx.parse_sess(), cx.cfg(), tts.into());
    // The `expand_expr` method is called so that any macro calls in the
    // parsed expression are expanded.

    let mut item = match parser.parse_impl_item() {
        Ok(s) => s,
        Err(mut diagnostic) => {
            diagnostic.emit();
            return DummyResult::any(sp);
        }
    };

    if let Err(mut diagnostic) = parser.expect(&token::Eof) {
        diagnostic.emit();
        return DummyResult::any(sp);
    }

    convert(&mut item.ident);
    MacEager::impl_items(SmallVector::one(item))
}

fn ty_snake_to_camel(cx: &mut ExtCtxt, sp: Span, tts: &[TokenTree]) -> Box<MacResult + 'static> {
    let mut parser = parse::new_parser_from_tts(cx.parse_sess(), cx.cfg(), tts.into());
    // The `expand_expr` method is called so that any macro calls in the
    // parsed expression are expanded.

    let mut ty = match parser.parse_ty_path() {
        Ok(s) => s,
        Err(mut diagnostic) => {
            diagnostic.emit();
            return DummyResult::any(sp);
        }
    };

    if let Err(mut diagnostic) = parser.expect(&token::Eof) {
        diagnostic.emit();
        return DummyResult::any(sp);
    }

    // Find the first non-underscore and add it capitalized.
    if let TyKind::Path(_, ref mut path) = ty {
        for segment in &mut path.segments {
            convert(&mut segment.identifier);
        }
    } else {
        unreachable!()
    }
    MacEager::ty(P(Ty {
        id: ast::DUMMY_NODE_ID,
        node: ty,
        span: sp,
    }))
}

fn convert(ident: &mut Ident) {
    let ident_str = ident.to_string();
    let mut camel_ty = String::new();

    // Find the first non-underscore and add it capitalized.
    let mut chars = ident_str.chars();

    // Find the first non-underscore char, uppercase it, and append it.
    // Guaranteed to succeed because all idents must have at least one non-underscore char.
    camel_ty.extend(chars.find(|&c| c != '_').unwrap().to_uppercase());

    // When we find an underscore, we remove it and capitalize the next char. To do this,
    // we need to ensure the next char is not another underscore.
    let mut chars = chars.coalesce(|c1, c2| {
        if c1 == '_' && c2 == '_' {
            Ok(c1)
        } else {
            Err((c1, c2))
        }
    });

    while let Some(c) = chars.next() {
        if c != '_' {
            camel_ty.push(c);
        } else {
            if let Some(c) = chars.next() {
                camel_ty.extend(c.to_uppercase());
            }
        }
    }

    *ident = Ident::with_empty_ctxt(token::intern(&camel_ty));
}

trait ParseTraitRef {
    fn parse_trait_ref(&mut self) -> PResult<TraitRef>;
}

impl<'a> ParseTraitRef for Parser<'a> {
    /// Parse a::B<String,i32>
    fn parse_trait_ref(&mut self) -> PResult<TraitRef> {
        Ok(TraitRef {
            path: try!(self.parse_path(PathStyle::Type)),
            ref_id: ast::DUMMY_NODE_ID,
        })
    }
}

#[plugin_registrar]
#[doc(hidden)]
pub fn plugin_registrar(reg: &mut Registry) {
    reg.register_macro("snake_to_camel", snake_to_camel);
    reg.register_macro("impl_snake_to_camel", impl_snake_to_camel);
    reg.register_macro("ty_snake_to_camel", ty_snake_to_camel);
}
