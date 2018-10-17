// Copyright 2018 Google LLC
//
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

extern crate proc_macro;
extern crate proc_macro2;
extern crate syn;
extern crate itertools;
extern crate quote;

use proc_macro::TokenStream;

use itertools::Itertools;
use quote::ToTokens;
use syn::{Ident, TraitItemType, TypePath, parse};
use proc_macro2::Span;
use std::str::FromStr;

#[proc_macro]
pub fn snake_to_camel(input: TokenStream) -> TokenStream {
    let i = input.clone();
    let mut assoc_type = parse::<TraitItemType>(input).unwrap_or_else(|_| panic!("Could not parse trait item from:\n{}", i));

    let old_ident = convert(&mut assoc_type.ident);

    for mut attr in &mut assoc_type.attrs {
        if let Some(pair) = attr.path.segments.first() {
            if pair.value().ident == "doc" {
                attr.tts = proc_macro2::TokenStream::from_str(&attr.tts.to_string().replace("{}", &old_ident)).unwrap();
            }
        }
    }

    assoc_type.into_token_stream().into()
}

#[proc_macro]
pub fn ty_snake_to_camel(input: TokenStream) -> TokenStream {
    let mut path = parse::<TypePath>(input).unwrap();

    // Only capitalize the final segment
    convert(&mut path.path
                     .segments
                     .last_mut()
                     .unwrap()
                     .into_value()
                     .ident);

    path.into_token_stream().into()
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
            } else if let Some(c) = chars.next() {
                camel_ty.extend(c.to_uppercase());
            }
        }
    }

    // The Fut suffix is hardcoded right now; this macro isn't really meant to be general-purpose.
    camel_ty.push_str("Fut");

    *ident = Ident::new(&camel_ty, Span::call_site());
    ident_str
}
