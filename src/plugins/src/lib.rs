#![feature(plugin_registrar, rustc_private)]

extern crate itertools;
extern crate rustc_plugin;
extern crate syntax;

use itertools::Itertools;
use rustc_plugin::Registry;
use syntax::ast::{self, Ident, TraitRef, Ty, TyKind};
use syntax::ext::base::{ExtCtxt, MacResult, DummyResult, MacEager};
use syntax::ext::quote::rt::Span;
use syntax::parse::{self, token, str_lit, PResult};
use syntax::parse::parser::{Parser, PathStyle};
use syntax::symbol::Symbol;
use syntax::ptr::P;
use syntax::tokenstream::{TokenTree, TokenStream};
use syntax::util::small_vector::SmallVector;

fn snake_to_camel(cx: &mut ExtCtxt, sp: Span, tts: &[TokenTree]) -> Box<MacResult + 'static> {
    let mut parser = parse::new_parser_from_tts(cx.parse_sess(), tts.into());
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

    let old_ident = convert(&mut item.ident);

    // As far as I know, it's not possible in macro_rules! to reference an $ident in a doc string,
    // so this is the hacky workaround.
    //
    // This code looks intimidating, but it's just iterating through the trait item's attributes
    // copying non-doc attributes, and modifying doc attributes such that replacing any {} in the
    // doc string instead holds the original, snake_case ident.
    let attrs: Vec<_> = item.attrs
        .drain(..)
        .map(|mut attr| {
            if !attr.is_sugared_doc {
                return attr;
            }

            // Getting at the underlying doc comment is surprisingly painful.
            // The call-chain goes something like:
            //
            //  - https://github.com/rust-lang/rust/blob/9c15de4fd59bee290848b5443c7e194fd5afb02c/src/libsyntax/attr.rs#L283
            //  - https://github.com/rust-lang/rust/blob/9c15de4fd59bee290848b5443c7e194fd5afb02c/src/libsyntax/attr.rs#L1067
            //  - https://github.com/rust-lang/rust/blob/9c15de4fd59bee290848b5443c7e194fd5afb02c/src/libsyntax/attr.rs#L1196
            //  - https://github.com/rust-lang/rust/blob/9c15de4fd59bee290848b5443c7e194fd5afb02c/src/libsyntax/parse/mod.rs#L399
            //  - https://github.com/rust-lang/rust/blob/9c15de4fd59bee290848b5443c7e194fd5afb02c/src/libsyntax/parse/mod.rs#L268
            //
            // Note that a docstring (i.e., something with is_sugared_doc) *always* has exactly two
            // tokens: an Eq followed by a Literal, where the Literal contains a Str_. We therefore
            // match against that, modifying the inner Str with our modified Symbol.
            let mut tokens = attr.tokens.clone().into_trees();
            if let Some(tt @ TokenTree::Token(_, token::Eq)) = tokens.next() {
                let mut docstr = tokens.next().expect("Docstrings must have literal docstring");
                if let TokenTree::Token(_, token::Literal(token::Str_(ref mut doc), _)) = docstr {
                    *doc = Symbol::intern(&str_lit(&doc.as_str()).replace("{}", &old_ident));
                } else {
                    unreachable!();
                }
                attr.tokens = TokenStream::concat(vec![tt.into(), docstr.into()]);
            } else {
                unreachable!();
            }

            attr
        })
        .collect();
    item.attrs.extend(attrs.into_iter());

    MacEager::trait_items(SmallVector::one(item))
}

fn impl_snake_to_camel(cx: &mut ExtCtxt, sp: Span, tts: &[TokenTree]) -> Box<MacResult + 'static> {
    let mut parser = parse::new_parser_from_tts(cx.parse_sess(), tts.into());
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
    let mut parser = parse::new_parser_from_tts(cx.parse_sess(), tts.into());
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

    // Only capitalize the final segment
    if let TyKind::Path(_, ref mut path) = ty {
        convert(&mut path.segments.last_mut().unwrap().identifier);
    } else {
        unreachable!()
    }
    MacEager::ty(P(Ty {
        id: ast::DUMMY_NODE_ID,
        node: ty,
        span: sp,
    }))
}

/// Converts an ident in-place to CamelCase and returns the previous ident.
fn convert(ident: &mut Ident) -> String {
    let ident_str = ident.to_string();
    let mut camel_ty = String::new();

    {
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
    }

    // The Fut suffix is hardcoded right now; this macro isn't really meant to be general-purpose.
    camel_ty.push_str("Fut");

    *ident = Ident::with_empty_ctxt(Symbol::intern(&camel_ty));
    ident_str
}

trait ParseTraitRef {
    fn parse_trait_ref(&mut self) -> PResult<TraitRef>;
}

impl<'a> ParseTraitRef for Parser<'a> {
    /// Parse a::B<String,i32>
    fn parse_trait_ref(&mut self) -> PResult<TraitRef> {
        Ok(TraitRef {
            path: self.parse_path(PathStyle::Type)?,
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
