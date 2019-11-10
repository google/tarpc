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
    ArgCaptured, Attribute, FnArg, Ident, Lit, LitBool, MetaNameValue, Pat, ReturnType, Token,
    Visibility,
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
        let mut rpcs = Vec::<RpcMethod>::new();
        while !content.is_empty() {
            rpcs.push(content.parse()?);
        }
        for rpc in &rpcs {
            if rpc.ident == "new" {
                return Err(syn::Error::new(
                    rpc.ident.span(),
                    format!(
                        "method name conflicts with generated fn `{}Client::new`",
                        ident
                    ),
                ));
            }
            if rpc.ident == "serve" {
                return Err(syn::Error::new(
                    rpc.ident.span(),
                    format!("method name conflicts with generated fn `{}::serve`", ident),
                ));
            }
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
                    _ => Err(syn::Error::new(
                        captured.pat.span(),
                        "patterns aren't allowed in RPC args",
                    )),
                },
                FnArg::SelfRef(self_ref) => Err(syn::Error::new(
                    self_ref.span(),
                    "method args cannot start with self",
                )),
                FnArg::SelfValue(self_val) => Err(syn::Error::new(
                    self_val.span(),
                    "method args cannot start with self",
                )),
                arg => Err(syn::Error::new(
                    arg.span(),
                    "method args must be explicitly typed patterns",
                )),
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

// If `derive_serde` meta item is not present, defaults to cfg!(feature = "serde1").
// `derive_serde` can only be true when serde1 is enabled.
struct DeriveSerde(bool);

impl Parse for DeriveSerde {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        if input.is_empty() {
            return Ok(DeriveSerde(cfg!(feature = "serde1")));
        }
        match input.parse::<MetaNameValue>()? {
            MetaNameValue {
                ref ident, ref lit, ..
            } if ident == "derive_serde" => match lit {
                Lit::Bool(LitBool { value: true, .. }) if cfg!(feature = "serde1") => {
                    Ok(DeriveSerde(true))
                }
                Lit::Bool(LitBool { value: true, .. }) => Err(syn::Error::new(
                    lit.span(),
                    "To enable serde, first enable the `serde1` feature of tarpc",
                )),
                Lit::Bool(LitBool { value: false, .. }) => Ok(DeriveSerde(false)),
                lit => Err(syn::Error::new(
                    lit.span(),
                    "`derive_serde` expects a value of type `bool`",
                )),
            },
            MetaNameValue { ident, .. } => Err(syn::Error::new(
                ident.span(),
                "tarpc::service only supports one meta item, `derive_serde = {bool}`",
            )),
        }
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
    let derive_serde = parse_macro_input!(attr as DeriveSerde);

    let Service {
        attrs,
        vis,
        ident,
        rpcs,
    } = parse_macro_input!(input as Service);
    let vis_repeated = std::iter::repeat(vis.clone());

    let camel_case_fn_names: Vec<String> = rpcs
        .iter()
        .map(|rpc| snake_to_camel(&rpc.ident.to_string()))
        .collect();
    let outputs: &Vec<TokenStream2> = &rpcs
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
    let camel_case_idents: &Vec<Ident> = &rpcs
        .iter()
        .zip(camel_case_fn_names.iter())
        .map(|(rpc, name)| Ident::new(name, rpc.ident.span()))
        .collect();
    let camel_case_idents2 = camel_case_idents;

    let args: &Vec<&Punctuated<ArgCaptured, Comma>> = &rpcs.iter().map(|rpc| &rpc.args).collect();
    let arg_vars: &Vec<Punctuated<&Pat, Comma>> = &args
        .iter()
        .map(|args| args.iter().map(|arg| &arg.pat).collect())
        .collect();
    let arg_vars2 = arg_vars;
    let method_names: &Vec<&Ident> = &rpcs.iter().map(|rpc| &rpc.ident).collect();
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
    let request_ident = Ident::new(&format!("{}Request", ident), ident.span());
    let request_ident_repeated = std::iter::repeat(request_ident.clone());
    let request_ident_repeated2 = request_ident_repeated.clone();
    let response_ident = Ident::new(&format!("{}Response", ident), ident.span());
    let response_ident_repeated = std::iter::repeat(response_ident.clone());
    let response_ident_repeated2 = response_ident_repeated.clone();
    let response_fut_name = format!("{}ResponseFut", ident);
    let response_fut_ident = Ident::new(&response_fut_name, ident.span());
    let response_fut_ident_repeated = std::iter::repeat(response_fut_ident.clone());
    let response_fut_ident_repeated2 = response_fut_ident_repeated.clone();
    let server_ident = Ident::new(&format!("Serve{}", ident), ident.span());

    let derive_serialize = if derive_serde.0 {
        quote!(#[derive(serde::Serialize, serde::Deserialize)])
    } else {
        quote!()
    };

    let tokens = quote! {
        #( #attrs )*
        #vis trait #ident: Clone {
            #( #types_and_fns )*

            /// Returns a serving function to use with tarpc::server::Server.
            fn serve(self) -> #server_ident<Self> {
                #server_ident { service: self }
            }
        }

        #[derive(Clone)]
        #vis struct #server_ident<S> {
            service: S,
        }

        impl<S> tarpc::server::Serve<#request_ident> for #server_ident<S>
            where S: #ident
        {
            type Resp = #response_ident;
            type Fut = #response_fut_ident<S>;

            fn serve(self, ctx: tarpc::context::Context, req: #request_ident) -> Self::Fut {
                match req {
                    #(
                        #request_ident_repeated::#camel_case_idents{ #arg_vars } => {
                            #response_fut_ident_repeated2::#camel_case_idents2(
                                #service_name_repeated2::#method_names(
                                    self.service, ctx, #arg_vars2))
                        }
                    )*
                }
            }
        }

        /// The request sent over the wire from the client to the server.
        #[derive(Debug)]
        #derive_serialize
        #vis enum #request_ident {
            #( #camel_case_idents{ #args } ),*
        }

        /// The response sent over the wire from the server to the client.
        #[derive(Debug)]
        #derive_serialize
        #vis enum #response_ident {
            #( #camel_case_idents(#outputs) ),*
        }

        /// A future resolving to a server response.
        #vis enum #response_fut_ident<S: #ident> {
            #( #camel_case_idents(<S as #service_name_repeated>::#future_types) ),*
        }

        impl<S: #ident> std::fmt::Debug for #response_fut_ident<S> {
            fn fmt(&self, fmt: &mut std::fmt::Formatter) -> std::fmt::Result {
                fmt.debug_struct(#response_fut_name).finish()
            }
        }

        impl<S: #ident> std::future::Future for #response_fut_ident<S> {
            type Output = #response_ident;

            fn poll(self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>)
                -> std::task::Poll<#response_ident>
            {
                unsafe {
                    match std::pin::Pin::get_unchecked_mut(self) {
                        #(
                            #response_fut_ident_repeated::#camel_case_idents(resp) =>
                                std::pin::Pin::new_unchecked(resp)
                                    .poll(cx)
                                    .map(#response_ident_repeated::#camel_case_idents2),
                        )*
                    }
                }
            }
        }

        #[allow(unused)]
        #[derive(Clone, Debug)]
        /// The client stub that makes RPC calls to the server. Exposes a Future interface.
        #vis struct #client_ident<C>(C);

        impl<C> From<C> for #client_ident<C>
             where for<'a> C: tarpc::Client<'a, #request_ident, Response = #response_ident>
        {
            fn from(client: C) -> Self {
                #client_ident(client)
            }
        }

        impl #client_ident<tarpc::client::channel::Channel<#request_ident, #response_ident>>
        {
            /// Returns a new client stub that sends requests over the given transport.
            #vis fn new<T>(config: tarpc::client::Config, transport: T)
                -> tarpc::client::NewClient<
                    Self,
                    tarpc::client::channel::RequestDispatch<#request_ident, #response_ident, T>>
            where
                T: tarpc::Transport<tarpc::ClientMessage<#request_ident>, tarpc::Response<#response_ident>>
            {
                let new_client = tarpc::client::new(config, transport);
                tarpc::client::NewClient {
                    client: #client_ident(new_client.client),
                    dispatch: new_client.dispatch,
                }
            }

        }

        impl<C> #client_ident<C>
            where C: for<'a> ::tarpc::client::Client<'a, #request_ident, Response = #response_ident>,
        {
            #(
                #[allow(unused)]
                #( #method_attrs )*
                #vis_repeated fn #method_names(&mut self, ctx: tarpc::context::Context, #args)
                    -> impl std::future::Future<Output = std::io::Result<#outputs>> + '_ {
                    let request = #request_ident_repeated2::#camel_case_idents { #arg_vars };
                    let resp = tarpc::Client::call(&mut self.0, ctx, request);
                    async move {
                        match resp.await? {
                            #response_ident_repeated2::#camel_case_idents2(msg) => std::result::Result::Ok(msg),
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
    let chars = ident_str.chars();

    let mut last_char_was_underscore = true;
    for c in chars {
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
