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
use quote::{format_ident, quote, ToTokens};
use syn::{
    braced,
    ext::IdentExt,
    parenthesized,
    parse::{Parse, ParseStream},
    parse_macro_input, parse_quote, parse_str,
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
        let ident: Ident = input.parse()?;
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
                    ident.unraw()
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

    let camel_case_fn_names: &Vec<_> = &rpcs
        .iter()
        .map(|rpc| snake_to_camel(&rpc.ident.unraw().to_string()))
        .collect();
    let args: &[&[PatType]] = &rpcs.iter().map(|rpc| &*rpc.args).collect::<Vec<_>>();
    let response_fut_name = &format!("{}ResponseFut", ident.unraw());
    let derive_serialize = if derive_serde.0 {
        Some(quote!(#[derive(serde::Serialize, serde::Deserialize)]))
    } else {
        None
    };

    ServiceGenerator {
        response_fut_name,
        service_ident: ident,
        server_ident: &format_ident!("Serve{}", ident),
        response_fut_ident: &Ident::new(&response_fut_name, ident.span()),
        client_ident: &format_ident!("{}Client", ident),
        request_ident: &format_ident!("{}Request", ident),
        response_ident: &format_ident!("{}Response", ident),
        vis,
        args,
        method_attrs: &rpcs.iter().map(|rpc| &*rpc.attrs).collect::<Vec<_>>(),
        method_idents: &rpcs.iter().map(|rpc| &rpc.ident).collect::<Vec<_>>(),
        attrs,
        rpcs,
        return_types: &rpcs
            .iter()
            .map(|rpc| match rpc.output {
                ReturnType::Type(_, ref ty) => ty,
                ReturnType::Default => unit_type,
            })
            .collect::<Vec<_>>(),
        arg_pats: &args
            .iter()
            .map(|args| args.iter().map(|arg| &*arg.pat).collect())
            .collect::<Vec<_>>(),
        camel_case_idents: &rpcs
            .iter()
            .zip(camel_case_fn_names.iter())
            .map(|(rpc, name)| Ident::new(name, rpc.ident.span()))
            .collect::<Vec<_>>(),
        future_types: &camel_case_fn_names
            .iter()
            .map(|name| parse_str(&format!("{}Fut", name)).unwrap())
            .collect::<Vec<_>>(),
        derive_serialize: derive_serialize.as_ref(),
    }
    .into_token_stream()
    .into()
}

// Things needed to generate the service items: trait, serve impl, request/response enums, and
// the client stub.
struct ServiceGenerator<'a> {
    service_ident: &'a Ident,
    server_ident: &'a Ident,
    response_fut_ident: &'a Ident,
    response_fut_name: &'a str,
    client_ident: &'a Ident,
    request_ident: &'a Ident,
    response_ident: &'a Ident,
    vis: &'a Visibility,
    attrs: &'a [Attribute],
    rpcs: &'a [RpcMethod],
    camel_case_idents: &'a [Ident],
    future_types: &'a [Type],
    method_idents: &'a [&'a Ident],
    method_attrs: &'a [&'a [Attribute]],
    args: &'a [&'a [PatType]],
    return_types: &'a [&'a Type],
    arg_pats: &'a [Vec<&'a Pat>],
    derive_serialize: Option<&'a TokenStream2>,
}

impl<'a> ServiceGenerator<'a> {
    fn trait_service(&self) -> TokenStream2 {
        let &Self {
            attrs,
            rpcs,
            vis,
            future_types,
            return_types,
            service_ident,
            server_ident,
            ..
        } = self;

        let types_and_fns = rpcs
            .iter()
            .zip(future_types.iter())
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

    fn struct_server(&self) -> TokenStream2 {
        let &Self {
            vis, server_ident, ..
        } = self;

        quote! {
            #[derive(Clone)]
            #vis struct #server_ident<S> {
                service: S,
            }
        }
    }

    fn impl_serve_for_server(&self) -> TokenStream2 {
        let &Self {
            request_ident,
            server_ident,
            service_ident,
            response_ident,
            response_fut_ident,
            camel_case_idents,
            arg_pats,
            method_idents,
            ..
        } = self;

        quote! {
            impl<S> tarpc::server::Serve<#request_ident> for #server_ident<S>
                where S: #service_ident
            {
                type Resp = #response_ident;
                type Fut = #response_fut_ident<S>;

                fn serve(self, ctx: tarpc::context::Context, req: #request_ident) -> Self::Fut {
                    match req {
                        #(
                            #request_ident::#camel_case_idents{ #( #arg_pats ),* } => {
                                #response_fut_ident::#camel_case_idents(
                                    #service_ident::#method_idents(
                                        self.service, ctx, #( #arg_pats ),*
                                    )
                                )
                            }
                        )*
                    }
                }
            }
        }
    }

    fn enum_request(&self) -> TokenStream2 {
        let &Self {
            derive_serialize,
            vis,
            request_ident,
            camel_case_idents,
            args,
            ..
        } = self;

        quote! {
            /// The request sent over the wire from the client to the server.
            #[derive(Debug)]
            #derive_serialize
            #vis enum #request_ident {
                #( #camel_case_idents{ #( #args ),* } ),*
            }
        }
    }

    fn enum_response(&self) -> TokenStream2 {
        let &Self {
            derive_serialize,
            vis,
            response_ident,
            camel_case_idents,
            return_types,
            ..
        } = self;

        quote! {
            /// The response sent over the wire from the server to the client.
            #[derive(Debug)]
            #derive_serialize
            #vis enum #response_ident {
                #( #camel_case_idents(#return_types) ),*
            }
        }
    }

    fn enum_response_future(&self) -> TokenStream2 {
        let &Self {
            vis,
            service_ident,
            response_fut_ident,
            camel_case_idents,
            future_types,
            ..
        } = self;

        quote! {
            /// A future resolving to a server response.
            #vis enum #response_fut_ident<S: #service_ident> {
                #( #camel_case_idents(<S as #service_ident>::#future_types) ),*
            }
        }
    }

    fn impl_debug_for_response_future(&self) -> TokenStream2 {
        let &Self {
            service_ident,
            response_fut_ident,
            response_fut_name,
            ..
        } = self;

        quote! {
            impl<S: #service_ident> std::fmt::Debug for #response_fut_ident<S> {
                fn fmt(&self, fmt: &mut std::fmt::Formatter) -> std::fmt::Result {
                    fmt.debug_struct(#response_fut_name).finish()
                }
            }
        }
    }

    fn impl_future_for_response_future(&self) -> TokenStream2 {
        let &Self {
            service_ident,
            response_fut_ident,
            response_ident,
            camel_case_idents,
            ..
        } = self;

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

    fn struct_client(&self) -> TokenStream2 {
        let &Self {
            vis,
            client_ident,
            request_ident,
            response_ident,
            ..
        } = self;

        quote! {
            #[allow(unused)]
            #[derive(Clone, Debug)]
            /// The client stub that makes RPC calls to the server. Exposes a Future interface.
            #vis struct #client_ident<C = tarpc::client::Channel<#request_ident, #response_ident>>(C);
        }
    }

    fn impl_from_for_client(&self) -> TokenStream2 {
        let &Self {
            client_ident,
            request_ident,
            response_ident,
            ..
        } = self;

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

    fn impl_client_new(&self) -> TokenStream2 {
        let &Self {
            client_ident,
            vis,
            request_ident,
            response_ident,
            ..
        } = self;

        quote! {
            impl #client_ident {
                /// Returns a new client stub that sends requests over the given transport.
                #vis fn new<T>(config: tarpc::client::Config, transport: T)
                    -> tarpc::client::NewClient<
                        Self,
                        tarpc::client::channel::RequestDispatch<#request_ident, #response_ident, T>
                    >
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

    fn impl_client_rpc_methods(&self) -> TokenStream2 {
        let &Self {
            client_ident,
            request_ident,
            response_ident,
            method_attrs,
            vis,
            method_idents,
            args,
            return_types,
            arg_pats,
            camel_case_idents,
            ..
        } = self;

        quote! {
            impl<C> #client_ident<C>
                where for<'a> C: tarpc::Client<'a, #request_ident, Response = #response_ident>
            {
                #(
                    #[allow(unused)]
                    #( #method_attrs )*
                    #vis fn #method_idents(&mut self, ctx: tarpc::context::Context, #( #args ),*)
                        -> impl std::future::Future<Output = std::io::Result<#return_types>> + '_ {
                        let request = #request_ident::#camel_case_idents { #( #arg_pats ),* };
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
}

impl<'a> ToTokens for ServiceGenerator<'a> {
    fn to_tokens(&self, output: &mut TokenStream2) {
        output.extend(vec![
            self.trait_service(),
            self.struct_server(),
            self.impl_serve_for_server(),
            self.enum_request(),
            self.enum_response(),
            self.enum_response_future(),
            self.impl_debug_for_response_future(),
            self.impl_future_for_response_future(),
            self.struct_client(),
            self.impl_from_for_client(),
            self.impl_client_new(),
            self.impl_client_rpc_methods(),
        ])
    }
}

fn snake_to_camel(ident_str: &str) -> String {
    let mut camel_ty = String::with_capacity(ident_str.len());

    let mut last_char_was_underscore = true;
    for c in ident_str.chars() {
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
