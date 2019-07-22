// Copyright 2018 Google LLC
//
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

#![recursion_limit = "512"]

extern crate proc_macro;
extern crate proc_macro2;
extern crate quote;
extern crate syn;

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{
    braced, parenthesized,
    parse::{Parse, ParseStream},
    parse_macro_input,
    punctuated::Punctuated,
    spanned::Spanned,
    token::Comma,
    ArgCaptured, Attribute, FnArg, Ident, Pat, ReturnType, Token, Visibility,
};

struct Service {
    attrs: Vec<Attribute>,
    vis: Visibility,
    ident: Ident,
    rpcs: Vec<RpcMethod>,
}

struct RpcMethod {
    attrs: Vec<Attribute>,
    ident: Ident,
    args: Punctuated<ArgCaptured, Comma>,
    output: ReturnType,
}

impl Parse for Service {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let attrs = input.call(Attribute::parse_outer)?;
        let vis = input.parse()?;
        input.parse::<Token![trait]>()?;
        let ident = input.parse()?;
        let content;
        braced!(content in input);
        let mut rpcs = Vec::new();
        while !content.is_empty() {
            rpcs.push(content.parse()?);
        }
        Ok(Service {
            attrs,
            vis,
            ident,
            rpcs,
        })
    }
}

impl Parse for RpcMethod {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let attrs = input.call(Attribute::parse_outer)?;
        input.parse::<Token![async]>()?;
        input.parse::<Token![fn]>()?;
        let ident = input.parse()?;
        let content;
        parenthesized!(content in input);
        let args: Punctuated<FnArg, Comma> = content.parse_terminated(FnArg::parse)?;
        let args = args
            .into_iter()
            .map(|arg| match arg {
                FnArg::Captured(captured) => match captured.pat {
                    Pat::Ident(_) => Ok(captured),
                    _ => {
                        return Err(syn::Error::new(
                            captured.pat.span(),
                            "patterns aren't allowed in RPC args",
                        ))
                    }
                },
                FnArg::SelfRef(self_ref) => {
                    return Err(syn::Error::new(
                        self_ref.span(),
                        "RPC args cannot start with self",
                    ))
                }
                FnArg::SelfValue(self_val) => {
                    return Err(syn::Error::new(
                        self_val.span(),
                        "RPC args cannot start with self",
                    ))
                }
                arg => {
                    return Err(syn::Error::new(
                        arg.span(),
                        "RPC args must be explicitly typed patterns",
                    ))
                }
            })
            .collect::<Result<_, _>>()?;
        let output = input.parse()?;
        input.parse::<Token![;]>()?;

        Ok(RpcMethod {
            attrs,
            ident,
            args,
            output,
        })
    }
}

/// Generates:
/// - service trait
/// - serve fn
/// - client stub struct
/// - new_stub client factory fn
/// - Request and Response enums
/// - ResponseFut Future
#[proc_macro_attribute]
pub fn service(attr: TokenStream, input: TokenStream) -> TokenStream {
    struct EmptyArgs;
    impl Parse for EmptyArgs {
        fn parse(_: ParseStream) -> syn::Result<Self> {
            Ok(EmptyArgs)
        }
    }
    parse_macro_input!(attr as EmptyArgs);

    let Service {
        attrs,
        vis,
        ident,
        rpcs,
    } = parse_macro_input!(input as Service);

    let camel_case_fn_names: Vec<String> = rpcs
        .iter()
        .map(|rpc| snake_to_camel(&rpc.ident.to_string()))
        .collect();
    let ref outputs: Vec<TokenStream2> = rpcs
        .iter()
        .map(|rpc| match rpc.output {
            ReturnType::Type(_, ref ty) => quote!(#ty),
            ReturnType::Default => quote!(()),
        })
        .collect();
    let future_types: Vec<Ident> = camel_case_fn_names
        .iter()
        .map(|name| Ident::new(&format!("{}Fut", name), ident.span()))
        .collect();
    let ref camel_case_idents: Vec<Ident> = rpcs
        .iter()
        .zip(camel_case_fn_names.iter())
        .map(|(rpc, name)| Ident::new(name, rpc.ident.span()))
        .collect();
    let camel_case_idents2 = camel_case_idents;

    let ref args: Vec<&Punctuated<ArgCaptured, Comma>> = rpcs.iter().map(|rpc| &rpc.args).collect();
    let ref arg_vars: Vec<Punctuated<&Pat, Comma>> = args
        .iter()
        .map(|args| args.iter().map(|arg| &arg.pat).collect())
        .collect();
    let arg_vars2 = arg_vars;
    let ref method_names: Vec<&Ident> = rpcs.iter().map(|rpc| &rpc.ident).collect();
    let method_attrs: Vec<_> = rpcs.iter().map(|rpc| &rpc.attrs).collect();

    let types_and_fns = rpcs
        .iter()
        .zip(future_types.iter())
        .zip(outputs.iter())
        .map(
            |(
                (
                    RpcMethod {
                        attrs, ident, args, ..
                    },
                    future_type,
                ),
                output,
            )| {
                let ty_doc = format!("The response future returned by {}.", ident);
                quote! {
                    #[doc = #ty_doc]
                    type #future_type: std::future::Future<Output = #output>;

                    #( #attrs )*
                    fn #ident(self, context: tarpc::context::Context, #args) -> Self::#future_type;
                }
            },
        );

    let service_name_repeated = std::iter::repeat(ident.clone());
    let service_name_repeated2 = service_name_repeated.clone();

    let client_ident = Ident::new(&format!("{}Client", ident), ident.span());

    #[cfg(feature = "serde1")]
    let derive_serialize = quote!(#[derive(serde::Serialize, serde::Deserialize)]);
    #[cfg(not(feature = "serde1"))]
    let derive_serialize = quote!();

    let tokens = quote! {
        #( #attrs )*
        #vis trait #ident: Clone + Send + 'static {
            #( #types_and_fns )*
        }

        /// The request sent over the wire from the client to the server.
        #[derive(Debug)]
        #derive_serialize
        #vis enum Request {
            #( #camel_case_idents{ #args } ),*
        }

        /// The response sent over the wire from the server to the client.
        #[derive(Debug)]
        #derive_serialize
        #vis enum Response {
            #( #camel_case_idents(#outputs) ),*
        }

        /// A future resolving to a server [`Response`].
        #vis enum ResponseFut<S: #ident> {
            #( #camel_case_idents(<S as #service_name_repeated>::#future_types) ),*
        }

        impl<S: #ident> std::fmt::Debug for ResponseFut<S> {
            fn fmt(&self, fmt: &mut std::fmt::Formatter) -> std::fmt::Result {
                fmt.debug_struct("ResponseFut").finish()
            }
        }

        impl<S: #ident> std::future::Future for ResponseFut<S> {
            type Output = std::io::Result<Response>;

            fn poll(self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>)
                -> std::task::Poll<std::io::Result<Response>>
            {
                unsafe {
                    match std::pin::Pin::get_unchecked_mut(self) {
                        #(
                            ResponseFut::#camel_case_idents(resp) =>
                                std::pin::Pin::new_unchecked(resp)
                                    .poll(cx)
                                    .map(Response::#camel_case_idents2)
                                    .map(Ok),
                        )*
                    }
                }
            }
        }

        /// Returns a serving function to use with tarpc::server::Server.
        #vis fn serve<S: #ident>(service: S)
            -> impl FnOnce(tarpc::context::Context, Request) -> ResponseFut<S> + Send + 'static + Clone {
            move |ctx, req| {
                match req {
                    #(
                        Request::#camel_case_idents{ #arg_vars } => {
                            ResponseFut::#camel_case_idents2(
                                #service_name_repeated2::#method_names(
                                    service.clone(), ctx, #arg_vars2))
                        }
                    )*
                }
            }
        }

        #[allow(unused)]
        #[derive(Clone, Debug)]
        /// The client stub that makes RPC calls to the server. Exposes a Future interface.
        #vis struct #client_ident<C = tarpc::client::Channel<Request, Response>>(C);

        /// Returns a new client stub that sends requests over the given transport.
        #vis async fn new_stub<T>(config: tarpc::client::Config, transport: T)
            -> std::io::Result<#client_ident>
        where
            T: tarpc::Transport<tarpc::ClientMessage<Request>, tarpc::Response<Response>> + Send + 'static,
        {
            Ok(#client_ident(tarpc::client::new(config, transport).await?))
        }

        impl<C> From<C> for #client_ident<C>
            where for <'a> C: tarpc::Client<'a, Request, Response = Response>
        {
            fn from(client: C) -> Self {
                #client_ident(client)
            }
        }

        impl<C> #client_ident<C>
            where for<'a> C: tarpc::Client<'a, Request, Response = Response>
        {
            #(
                #[allow(unused)]
                #( #method_attrs )*
                pub fn #method_names(&mut self, ctx: tarpc::context::Context, #args)
                    -> impl std::future::Future<Output = std::io::Result<#outputs>> + '_ {
                    let request = Request::#camel_case_idents { #arg_vars };
                    let resp = tarpc::Client::call(&mut self.0, ctx, request);
                    async move {
                        match resp.await? {
                            Response::#camel_case_idents2(msg) => std::result::Result::Ok(msg),
                            _ => unreachable!(),
                        }
                    }
                }
            )*
        }
    };

    tokens.into()
}

fn snake_to_camel(ident_str: &str) -> String {
    let mut camel_ty = String::new();
    let mut chars = ident_str.chars();

    let mut last_char_was_underscore = true;
    while let Some(c) = chars.next() {
        match c {
            '_' => last_char_was_underscore = true,
            c if last_char_was_underscore => {
                camel_ty.extend(c.to_uppercase());
                last_char_was_underscore = false;
            }
            c => camel_ty.extend(c.to_lowercase()),
        }
    }

    camel_ty
}

#[test]
fn snake_to_camel_basic() {
    assert_eq!(snake_to_camel("abc_def"), "AbcDef");
}

#[test]
fn snake_to_camel_underscore_suffix() {
    assert_eq!(snake_to_camel("abc_def_"), "AbcDef");
}

#[test]
fn snake_to_camel_underscore_prefix() {
    assert_eq!(snake_to_camel("_abc_def"), "AbcDef");
}

#[test]
fn snake_to_camel_underscore_consecutive() {
    assert_eq!(snake_to_camel("abc__def"), "AbcDef");
}

#[test]
fn snake_to_camel_capital_in_middle() {
    assert_eq!(snake_to_camel("aBc_dEf"), "AbcDef");
}
