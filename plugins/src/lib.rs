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
use quote::{format_ident, quote};
use syn::{
    braced, parenthesized,
    parse::{Parse, ParseStream},
    parse_macro_input, parse_quote,
    punctuated::Punctuated,
    token::Comma,
    Attribute, FnArg, Ident, Lit, LitBool, MetaNameValue, Pat, PatType, ReturnType, Token, Type,
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
    args: Vec<PatType>,
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
                return Err(input.error(format!(
                    "method name conflicts with generated fn `{}Client::new`",
                    ident
                )));
            }
            if rpc.ident == "serve" {
                return Err(input.error(format!(
                    "method name conflicts with generated fn `{}::serve`",
                    ident
                )));
            }
        }
        Ok(Self {
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
                FnArg::Typed(captured) => match *captured.pat {
                    Pat::Ident(_) => Ok(captured),
                    _ => Err(input.error("patterns aren't allowed in RPC args")),
                },
                FnArg::Receiver(_) => Err(input.error("method args cannot start with self")),
            })
            .collect::<Result<_, _>>()?;
        let output = input.parse()?;
        input.parse::<Token![;]>()?;

        Ok(Self {
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
            return Ok(Self(cfg!(feature = "serde1")));
        }
        match input.parse::<MetaNameValue>()? {
            MetaNameValue {
                ref path, ref lit, ..
            } if path.segments.len() == 1
                && path.segments.first().unwrap().ident == "derive_serde" =>
            {
                match lit {
                    Lit::Bool(LitBool { value: true, .. }) if cfg!(feature = "serde1") => {
                        Ok(Self(true))
                    }
                    Lit::Bool(LitBool { value: true, .. }) => {
                        Err(input
                            .error("To enable serde, first enable the `serde1` feature of tarpc"))
                    }
                    Lit::Bool(LitBool { value: false, .. }) => Ok(Self(false)),
                    _ => Err(input.error("`derive_serde` expects a value of type `bool`")),
                }
            }
            _ => {
                Err(input
                    .error("tarpc::service only supports one meta item, `derive_serde = {bool}`"))
            }
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
    let unit_type: &Type = &parse_quote!(());
    let Service {
        ref attrs,
        ref vis,
        ref ident,
        ref rpcs,
    } = parse_macro_input!(input as Service);

    let camel_case_fn_names: &[String] = &rpcs
        .iter()
        .map(|rpc| snake_to_camel(&rpc.ident.to_string()))
        .collect::<Vec<_>>();
    let args: &[&[PatType]] = &rpcs.iter().map(|rpc| &*rpc.args).collect::<Vec<_>>();
    let response_fut_name = &format!("{}ResponseFut", ident);
    let derive_serialize = quote!(#[derive(serde::Serialize, serde::Deserialize)]);

    let gen_args = &GenArgs {
        attrs,
        vis,
        rpcs,
        args,
        response_fut_name,
        service_ident: ident,
        server_ident: &format_ident!("Serve{}", ident),
        response_fut_ident: &Ident::new(&response_fut_name, ident.span()),
        client_ident: &format_ident!("{}Client", ident),
        request_ident: &format_ident!("{}Request", ident),
        response_ident: &format_ident!("{}Response", ident),
        method_attrs: &rpcs.iter().map(|rpc| &*rpc.attrs).collect::<Vec<_>>(),
        method_names: &rpcs.iter().map(|rpc| &rpc.ident).collect::<Vec<_>>(),
        return_types: &rpcs
            .iter()
            .map(|rpc| match rpc.output {
                ReturnType::Type(_, ref ty) => ty,
                ReturnType::Default => unit_type,
            })
            .collect::<Vec<_>>(),
        arg_vars: &args
            .iter()
            .map(|args| args.iter().map(|arg| &*arg.pat).collect())
            .collect::<Vec<_>>(),
        camel_case_idents: &rpcs
            .iter()
            .zip(camel_case_fn_names.iter())
            .map(|(rpc, name)| Ident::new(name, rpc.ident.span()))
            .collect::<Vec<_>>(),
        future_idents: &camel_case_fn_names
            .iter()
            .map(|name| format_ident!("{}Fut", name))
            .collect::<Vec<_>>(),
        derive_serialize: if derive_serde.0 {
            Some(&derive_serialize)
        } else {
            None
        },
    };

    let tokens = vec![
        trait_service(gen_args),
        struct_server(gen_args),
        impl_serve_for_server(gen_args),
        enum_request(gen_args),
        enum_response(gen_args),
        enum_response_future(gen_args),
        impl_debug_for_response_future(gen_args),
        impl_future_for_response_future(gen_args),
        struct_client(gen_args),
        impl_from_for_client(gen_args),
        impl_client_new(gen_args),
        impl_client_rpc_methods(gen_args),
    ];

    tokens
        .into_iter()
        .collect::<proc_macro2::TokenStream>()
        .into()
}

// Things needed to generate the service items: trait, serve impl, request/response enums, and
// the client stub.
struct GenArgs<'a> {
    attrs: &'a [Attribute],
    rpcs: &'a [RpcMethod],
    service_ident: &'a Ident,
    server_ident: &'a Ident,
    response_fut_ident: &'a Ident,
    response_fut_name: &'a str,
    client_ident: &'a Ident,
    request_ident: &'a Ident,
    response_ident: &'a Ident,
    method_attrs: &'a [&'a [Attribute]],
    vis: &'a Visibility,
    method_names: &'a [&'a Ident],
    args: &'a [&'a [PatType]],
    return_types: &'a [&'a Type],
    arg_vars: &'a [Vec<&'a Pat>],
    camel_case_idents: &'a [Ident],
    future_idents: &'a [Ident],
    derive_serialize: Option<&'a proc_macro2::TokenStream>,
}

fn trait_service(
    &GenArgs {
        attrs,
        rpcs,
        vis,
        future_idents,
        return_types,
        service_ident,
        server_ident,
        ..
    }: &GenArgs,
) -> proc_macro2::TokenStream {
    let types_and_fns = rpcs
        .iter()
        .zip(future_idents.iter())
        .zip(return_types.iter())
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
                    fn #ident(self, context: tarpc::context::Context, #( #args ),*) -> Self::#future_type;
                }
            },
        );

    quote! {
        #( #attrs )*
        #vis trait #service_ident: Clone {
            #( #types_and_fns )*

            /// Returns a serving function to use with tarpc::server::Server.
            fn serve(self) -> #server_ident<Self> {
                #server_ident { service: self }
            }
        }
    }
}

fn struct_server(
    &GenArgs {
        vis, server_ident, ..
    }: &GenArgs,
) -> proc_macro2::TokenStream {
    quote! {
        #[derive(Clone)]
        #vis struct #server_ident<S> {
            service: S,
        }
    }
}

fn impl_serve_for_server(
    &GenArgs {
        request_ident,
        server_ident,
        service_ident,
        response_ident,
        response_fut_ident,
        camel_case_idents,
        arg_vars,
        method_names,
        ..
    }: &GenArgs,
) -> proc_macro2::TokenStream {
    quote! {
        impl<S> tarpc::server::Serve<#request_ident> for #server_ident<S>
            where S: #service_ident
        {
            type Resp = #response_ident;
            type Fut = #response_fut_ident<S>;

            fn serve(self, ctx: tarpc::context::Context, req: #request_ident) -> Self::Fut {
                match req {
                    #(
                        #request_ident::#camel_case_idents{ #( #arg_vars ),* } => {
                            #response_fut_ident::#camel_case_idents(
                                #service_ident::#method_names(
                                    self.service, ctx, #( #arg_vars ),*))
                        }
                    )*
                }
            }
        }
    }
}

fn enum_request(
    &GenArgs {
        derive_serialize,
        vis,
        request_ident,
        camel_case_idents,
        args,
        ..
    }: &GenArgs,
) -> proc_macro2::TokenStream {
    quote! {
        /// The request sent over the wire from the client to the server.
        #[derive(Debug)]
        #derive_serialize
        #vis enum #request_ident {
            #( #camel_case_idents{ #( #args ),* } ),*
        }
    }
}

fn enum_response(
    &GenArgs {
        derive_serialize,
        vis,
        response_ident,
        camel_case_idents,
        return_types,
        ..
    }: &GenArgs,
) -> proc_macro2::TokenStream {
    quote! {
        /// The response sent over the wire from the server to the client.
        #[derive(Debug)]
        #derive_serialize
        #vis enum #response_ident {
            #( #camel_case_idents(#return_types) ),*
        }
    }
}

fn enum_response_future(
    &GenArgs {
        vis,
        service_ident,
        response_fut_ident,
        camel_case_idents,
        future_idents,
        ..
    }: &GenArgs,
) -> proc_macro2::TokenStream {
    quote! {
        /// A future resolving to a server response.
        #vis enum #response_fut_ident<S: #service_ident> {
            #( #camel_case_idents(<S as #service_ident>::#future_idents) ),*
        }
    }
}

fn impl_debug_for_response_future(
    &GenArgs {
        service_ident,
        response_fut_ident,
        response_fut_name,
        ..
    }: &GenArgs,
) -> proc_macro2::TokenStream {
    quote! {
        impl<S: #service_ident> std::fmt::Debug for #response_fut_ident<S> {
            fn fmt(&self, fmt: &mut std::fmt::Formatter) -> std::fmt::Result {
                fmt.debug_struct(#response_fut_name).finish()
            }
        }
    }
}

fn impl_future_for_response_future(
    &GenArgs {
        service_ident,
        response_fut_ident,
        response_ident,
        camel_case_idents,
        ..
    }: &GenArgs,
) -> proc_macro2::TokenStream {
    quote! {
        impl<S: #service_ident> std::future::Future for #response_fut_ident<S> {
            type Output = #response_ident;

            fn poll(self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>)
                -> std::task::Poll<#response_ident>
            {
                unsafe {
                    match std::pin::Pin::get_unchecked_mut(self) {
                        #(
                            #response_fut_ident::#camel_case_idents(resp) =>
                                std::pin::Pin::new_unchecked(resp)
                                    .poll(cx)
                                    .map(#response_ident::#camel_case_idents),
                        )*
                    }
                }
            }
        }
    }
}

fn struct_client(
    &GenArgs {
        vis,
        client_ident,
        request_ident,
        response_ident,
        ..
    }: &GenArgs,
) -> proc_macro2::TokenStream {
    quote! {
        #[allow(unused)]
        #[derive(Clone, Debug)]
        /// The client stub that makes RPC calls to the server. Exposes a Future interface.
        #vis struct #client_ident<C = tarpc::client::Channel<#request_ident, #response_ident>>(C);
    }
}

fn impl_from_for_client(
    &GenArgs {
        client_ident,
        request_ident,
        response_ident,
        ..
    }: &GenArgs,
) -> proc_macro2::TokenStream {
    quote! {
        impl<C> From<C> for #client_ident<C>
            where for <'a> C: tarpc::Client<'a, #request_ident, Response = #response_ident>
        {
            fn from(client: C) -> Self {
                #client_ident(client)
            }
        }
    }
}

fn impl_client_new(
    &GenArgs {
        client_ident,
        vis,
        request_ident,
        response_ident,
        ..
    }: &GenArgs,
) -> proc_macro2::TokenStream {
    quote! {
        impl #client_ident {
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
    }
}

fn impl_client_rpc_methods(
    &GenArgs {
        client_ident,
        request_ident,
        response_ident,
        method_attrs,
        vis,
        method_names,
        args,
        return_types,
        arg_vars,
        camel_case_idents,
        ..
    }: &GenArgs,
) -> proc_macro2::TokenStream {
    quote! {
        impl<C> #client_ident<C>
            where for<'a> C: tarpc::Client<'a, #request_ident, Response = #response_ident>
        {
            #(
                #[allow(unused)]
                #( #method_attrs )*
                #vis fn #method_names(&mut self, ctx: tarpc::context::Context, #( #args ),*)
                    -> impl std::future::Future<Output = std::io::Result<#return_types>> + '_ {
                    let request = #request_ident::#camel_case_idents { #( #arg_vars ),* };
                    let resp = tarpc::Client::call(&mut self.0, ctx, request);
                    async move {
                        match resp.await? {
                            #response_ident::#camel_case_idents(msg) => std::result::Result::Ok(msg),
                            _ => unreachable!(),
                        }
                    }
                }
            )*
        }
    }
}

fn snake_to_camel(ident_str: &str) -> String {
    let chars = ident_str.chars();
    let mut camel_ty = String::with_capacity(ident_str.len());

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

    camel_ty.shrink_to_fit();
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
