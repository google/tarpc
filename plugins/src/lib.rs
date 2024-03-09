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
    parse_macro_input, parse_quote,
    spanned::Spanned,
    token::Comma,
    AttrStyle, Attribute, Expr, FnArg, Ident, Lit, LitBool, MetaNameValue, Pat, PatType, Path,
    ReturnType, Token, Type, Visibility,
};

/// Accumulates multiple errors into a result.
/// Only use this for recoverable errors, i.e. non-parse errors. Fatal errors should early exit to
/// avoid further complications.
macro_rules! extend_errors {
    ($errors: ident, $e: expr) => {
        match $errors {
            Ok(_) => $errors = Err($e),
            Err(ref mut errors) => errors.extend($e),
        }
    };
}

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
        let mut ident_errors = Ok(());
        for rpc in &rpcs {
            if rpc.ident == "new" {
                extend_errors!(
                    ident_errors,
                    syn::Error::new(
                        rpc.ident.span(),
                        format!(
                            "method name conflicts with generated fn `{}Client::new`",
                            ident.unraw()
                        )
                    )
                );
            }
            if rpc.ident == "serve" {
                extend_errors!(
                    ident_errors,
                    syn::Error::new(
                        rpc.ident.span(),
                        format!("method name conflicts with generated fn `{ident}::serve`")
                    )
                );
            }
        }
        ident_errors?;

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
        let mut args = Vec::new();
        let mut errors = Ok(());
        for arg in content.parse_terminated(FnArg::parse, Comma)? {
            match arg {
                FnArg::Typed(captured) if matches!(&*captured.pat, Pat::Ident(_)) => {
                    args.push(captured);
                }
                FnArg::Typed(captured) => {
                    extend_errors!(
                        errors,
                        syn::Error::new(captured.pat.span(), "patterns aren't allowed in RPC args")
                    );
                }
                FnArg::Receiver(_) => {
                    extend_errors!(
                        errors,
                        syn::Error::new(arg.span(), "method args cannot start with self")
                    );
                }
            }
        }
        errors?;
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

#[derive(Default)]
struct DeriveMeta {
    derive: Option<Derive>,
    warnings: Vec<TokenStream2>,
}

impl DeriveMeta {
    fn with_derives(mut self, new: Vec<Path>) -> Self {
        match self.derive.as_mut() {
            Some(Derive::Explicit(old)) => old.extend(new),
            _ => self.derive = Some(Derive::Explicit(new)),
        }

        self
    }
}

enum Derive {
    Explicit(Vec<Path>),
    Serde(bool),
}

impl Parse for DeriveMeta {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let mut result = Ok(DeriveMeta::default());

        let mut derives = Vec::new();
        let mut derive_serde = Vec::new();
        let mut has_derive_serde = false;
        let mut has_explicit_derives = false;

        let meta_items = input.parse_terminated(MetaNameValue::parse, Comma)?;
        for meta in meta_items {
            if meta.path.segments.len() != 1 {
                extend_errors!(
                    result,
                    syn::Error::new(
                        meta.span(),
                        "tarpc::service does not support this meta item"
                    )
                );
                continue;
            }
            let segment = meta.path.segments.first().unwrap();
            if segment.ident == "derive" {
                has_explicit_derives = true;
                let Expr::Array(ref array) = meta.value else {
                    extend_errors!(
                        result,
                        syn::Error::new(
                            meta.span(),
                            "tarpc::service does not support this meta item"
                        )
                    );
                    continue;
                };

                let paths = array
                    .elems
                    .iter()
                    .filter_map(|e| {
                        if let Expr::Path(path) = e {
                            Some(path.path.clone())
                        } else {
                            extend_errors!(
                                result,
                                syn::Error::new(e.span(), "Expected Path or Type")
                            );
                            None
                        }
                    })
                    .collect::<Vec<_>>();

                result = result.map(|d| d.with_derives(paths));
                derives.push(meta);
            } else if segment.ident == "derive_serde" {
                has_derive_serde = true;
                let Expr::Lit(expr_lit) = &meta.value else {
                    extend_errors!(
                        result,
                        syn::Error::new(meta.value.span(), "expected literal")
                    );
                    continue;
                };
                match expr_lit.lit {
                    Lit::Bool(LitBool { value: true, .. }) if cfg!(feature = "serde1") => {
                        result = result.map(|d| DeriveMeta {
                            derive: Some(Derive::Serde(true)),
                            ..d
                        })
                    }
                    Lit::Bool(LitBool { value: true, .. }) => {
                        extend_errors!(
                            result,
                            syn::Error::new(
                                meta.span(),
                                "To enable serde, first enable the `serde1` feature of tarpc"
                            )
                        );
                    }
                    Lit::Bool(LitBool { value: false, .. }) => {
                        result = result.map(|d| DeriveMeta {
                            derive: Some(Derive::Serde(false)),
                            ..d
                        })
                    }
                    _ => extend_errors!(
                        result,
                        syn::Error::new(
                            expr_lit.lit.span(),
                            "`derive_serde` expects a value of type `bool`"
                        )
                    ),
                }
                derive_serde.push(meta);
            } else {
                extend_errors!(
                    result,
                    syn::Error::new(
                        meta.span(),
                        "tarpc::service does not support this meta item"
                    )
                );
                continue;
            }
        }

        if has_derive_serde {
            let deprecation_hack = quote! {
                const _: () = {
                    #[deprecated(
                        note = "\nThe form `tarpc::service(derive_serde = true)` is deprecated.\
                        \nUse `tarpc::service(derive = [Serialize, Deserialize])`."
                    )]
                    const DEPRECATED_SYNTAX: () = ();
                    let _ = DEPRECATED_SYNTAX;
                };
            };

            result = result.map(|mut d| {
                d.warnings.push(deprecation_hack.to_token_stream());
                d
            });
        }

        if has_explicit_derives & has_derive_serde {
            extend_errors!(
                result,
                syn::Error::new(
                    input.span(),
                    "tarpc does not support `derive_serde` and `derive` at the same time"
                )
            );
        }

        if derive_serde.len() > 1 {
            for (i, derive_serde) in derive_serde.iter().enumerate() {
                extend_errors!(
                    result,
                    syn::Error::new(
                        derive_serde.span(),
                        format!(
                            "`derive_serde` appears more than once (occurrence #{})",
                            i + 1
                        )
                    )
                );
            }
        }

        if derives.len() > 1 {
            for (i, derive) in derives.iter().enumerate() {
                extend_errors!(
                    result,
                    syn::Error::new(
                        derive.span(),
                        format!("`derive` appears more than once (occurrence #{})", i + 1)
                    )
                );
            }
        }

        result
    }
}

/// A helper attribute to avoid a direct dependency on Serde.
///
/// Adds the following annotations to the annotated item:
///
/// ```rust
/// #[derive(::tarpc::serde::Serialize, ::tarpc::serde::Deserialize)]
/// #[serde(crate = "tarpc::serde")]
/// # struct Foo;
/// ```
#[proc_macro_attribute]
#[cfg(feature = "serde1")]
pub fn derive_serde(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let mut gen: proc_macro2::TokenStream = quote! {
        #[derive(::tarpc::serde::Serialize, ::tarpc::serde::Deserialize)]
        #[serde(crate = "::tarpc::serde")]
    };
    gen.extend(proc_macro2::TokenStream::from(item));
    proc_macro::TokenStream::from(gen)
}

fn collect_cfg_attrs(rpcs: &[RpcMethod]) -> Vec<Vec<&Attribute>> {
    rpcs.iter()
        .map(|rpc| {
            rpc.attrs
                .iter()
                .filter(|att| {
                    att.style == AttrStyle::Outer
                        && match &att.meta {
                            syn::Meta::List(syn::MetaList { path, .. }) => {
                                path.get_ident() == Some(&Ident::new("cfg", rpc.ident.span()))
                            }
                            _ => false,
                        }
                })
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>()
}

/// This macro generates the machinery used by both the client and server.
///
/// Namely, it produces:
///   - a serve fn inside the trait
///   - client stub struct
///   - Request and Response enums
///
/// # Example
///
/// ```no_run
/// use tarpc::{client, transport, service, server::{self, Channel}, context::Context};
///
/// #[service]
/// pub trait Calculator {
///     async fn add(a: i32, b: i32) -> i32;
/// }
///
/// // The request type looks like the following.
/// // Note, you don't have to interact with this type directly outside
/// // of testing, it is used by the client and server implementation
/// let req = CalculatorRequest::Add {a: 5, b: 7};
///
/// // This would be the associated response, again you don't ofent use this,
/// // it is only shown for educational purposes.
/// let resp = CalculatorResponse::Add(12);
///
/// // This could be any transport.
/// let (client_side, server_side) = transport::channel::unbounded();
///
/// // A client can be made like so:
/// let client = CalculatorClient::new(client::Config::default(), client_side);
///
/// // And a server like so:
/// #[derive(Clone)]
/// struct CalculatorServer;
/// impl Calculator for CalculatorServer {
///     async fn add(self, context: Context, a: i32, b: i32) -> i32 {
///         a + b
///     }
/// }
///
/// // You would usually spawn on an async runtime.
/// let server = server::BaseChannel::with_defaults(server_side);
/// let _ = server.execute(CalculatorServer.serve());
/// ```
#[proc_macro_attribute]
pub fn service(attr: TokenStream, input: TokenStream) -> TokenStream {
    let derive_meta = parse_macro_input!(attr as DeriveMeta);
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

    let derives = match derive_meta.derive.as_ref() {
        Some(Derive::Explicit(paths)) => {
            if !paths.is_empty() {
                Some(quote! {
                    #[derive(
                        #(
                            #paths
                        ),*
                    )]
                })
            } else {
                None
            }
        }
        Some(Derive::Serde(serde)) => {
            if *serde {
                Some(quote! {
                    #[derive(::tarpc::serde::Serialize, ::tarpc::serde::Deserialize)]
                    #[serde(crate = "::tarpc::serde")]
                })
            } else {
                None
            }
        }
        None => {
            if cfg!(feature = "serde1") {
                Some(quote! {
                    #[derive(::tarpc::serde::Serialize, ::tarpc::serde::Deserialize)]
                    #[serde(crate = "::tarpc::serde")]
                })
            } else {
                None
            }
        }
    };

    let methods = rpcs.iter().map(|rpc| &rpc.ident).collect::<Vec<_>>();
    let request_names = methods
        .iter()
        .map(|m| format!("{ident}.{m}"))
        .collect::<Vec<_>>();

    ServiceGenerator {
        service_ident: ident,
        client_stub_ident: &format_ident!("{}Stub", ident),
        server_ident: &format_ident!("Serve{}", ident),
        client_ident: &format_ident!("{}Client", ident),
        request_ident: &format_ident!("{}Request", ident),
        response_ident: &format_ident!("{}Response", ident),
        vis,
        args,
        method_attrs: &rpcs.iter().map(|rpc| &*rpc.attrs).collect::<Vec<_>>(),
        method_cfgs: &collect_cfg_attrs(rpcs),
        method_idents: &methods,
        request_names: &request_names,
        attrs,
        rpcs,
        return_types: &rpcs
            .iter()
            .map(|rpc| match rpc.output {
                ReturnType::Type(_, ref ty) => ty.as_ref(),
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
        derives: derives.as_ref(),
        warnings: &derive_meta.warnings,
    }
    .into_token_stream()
    .into()
}

// Things needed to generate the service items: trait, serve impl, request/response enums, and
// the client stub.
struct ServiceGenerator<'a> {
    service_ident: &'a Ident,
    client_stub_ident: &'a Ident,
    server_ident: &'a Ident,
    client_ident: &'a Ident,
    request_ident: &'a Ident,
    response_ident: &'a Ident,
    vis: &'a Visibility,
    attrs: &'a [Attribute],
    rpcs: &'a [RpcMethod],
    camel_case_idents: &'a [Ident],
    method_idents: &'a [&'a Ident],
    request_names: &'a [String],
    method_attrs: &'a [&'a [Attribute]],
    method_cfgs: &'a [Vec<&'a Attribute>],
    args: &'a [&'a [PatType]],
    return_types: &'a [&'a Type],
    arg_pats: &'a [Vec<&'a Pat>],
    derives: Option<&'a TokenStream2>,
    warnings: &'a [TokenStream2],
}

impl<'a> ServiceGenerator<'a> {
    fn trait_service(&self) -> TokenStream2 {
        let &Self {
            attrs,
            rpcs,
            vis,
            return_types,
            service_ident,
            client_stub_ident,
            request_ident,
            response_ident,
            server_ident,
            ..
        } = self;

        let rpc_fns = rpcs
            .iter()
            .zip(return_types.iter())
            .map(
                |(
                     RpcMethod {
                         attrs, ident, args, ..
                     },
                     output,
                 )| {
                    quote! {
                        #( #attrs )*
                        async fn #ident(self, context: ::tarpc::context::Context, #( #args ),*) -> #output;
                    }
                },
            );

        let stub_doc = format!("The stub trait for service [`{service_ident}`].");
        quote! {
            #( #attrs )*
            #vis trait #service_ident: ::core::marker::Sized {
                #( #rpc_fns )*

                /// Returns a serving function to use with
                /// [InFlightRequest::execute](::tarpc::server::InFlightRequest::execute).
                fn serve(self) -> #server_ident<Self> {
                    #server_ident { service: self }
                }
            }

            #[doc = #stub_doc]
            #vis trait #client_stub_ident: ::tarpc::client::stub::Stub<Req = #request_ident, Resp = #response_ident> {
            }

            impl<S> #client_stub_ident for S
                where S: ::tarpc::client::stub::Stub<Req = #request_ident, Resp = #response_ident>
            {
            }
        }
    }

    fn struct_server(&self) -> TokenStream2 {
        let &Self {
            vis, server_ident, ..
        } = self;

        quote! {
            /// A serving function to use with [::tarpc::server::InFlightRequest::execute].
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
            camel_case_idents,
            arg_pats,
            method_idents,
            method_cfgs,
            ..
        } = self;

        quote! {
            impl<S> ::tarpc::server::Serve for #server_ident<S>
                where S: #service_ident
            {
                type Req = #request_ident;
                type Resp = #response_ident;


                async fn serve(self, ctx: ::tarpc::context::Context, req: #request_ident)
                    -> ::core::result::Result<#response_ident, ::tarpc::ServerError> {
                    match req {
                        #(
                            #( #method_cfgs )*
                            #request_ident::#camel_case_idents{ #( #arg_pats ),* } => {
                                ::core::result::Result::Ok(#response_ident::#camel_case_idents(
                                    #service_ident::#method_idents(
                                        self.service, ctx, #( #arg_pats ),*
                                    ).await
                                ))
                            }
                        )*
                    }
                }
            }
        }
    }

    fn enum_request(&self) -> TokenStream2 {
        let &Self {
            derives,
            vis,
            request_ident,
            camel_case_idents,
            args,
            request_names,
            method_cfgs,
            ..
        } = self;

        quote! {
            /// The request sent over the wire from the client to the server.
            #[allow(missing_docs)]
            #[derive(Debug)]
            #derives
            #vis enum #request_ident {
                #(
                    #( #method_cfgs )*
                    #camel_case_idents{ #( #args ),* }
                ),*
            }
            impl ::tarpc::RequestName for #request_ident {
                fn name(&self) -> &'static str {
                    match self {
                        #(
                            #( #method_cfgs )*
                            #request_ident::#camel_case_idents{..} => {
                                #request_names
                            }
                        )*
                    }
                }
            }
        }
    }

    fn enum_response(&self) -> TokenStream2 {
        let &Self {
            derives,
            vis,
            response_ident,
            camel_case_idents,
            return_types,
            ..
        } = self;

        quote! {
            /// The response sent over the wire from the server to the client.
            #[allow(missing_docs)]
            #[derive(Debug)]
            #derives
            #vis enum #response_ident {
                #( #camel_case_idents(#return_types) ),*
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
            /// The client stub that makes RPC calls to the server. All request methods return
            /// [Futures](::core::future::Future).
            #vis struct #client_ident<
                Stub = ::tarpc::client::Channel<#request_ident, #response_ident>
            >(Stub);
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
                #vis fn new<T>(config: ::tarpc::client::Config, transport: T)
                    -> ::tarpc::client::NewClient<
                        Self,
                        ::tarpc::client::RequestDispatch<#request_ident, #response_ident, T>
                    >
                where
                    T: ::tarpc::Transport<::tarpc::ClientMessage<#request_ident>, ::tarpc::Response<#response_ident>>
                {
                    let new_client = ::tarpc::client::new(config, transport);
                    ::tarpc::client::NewClient {
                        client: #client_ident(new_client.client),
                        dispatch: new_client.dispatch,
                    }
                }
            }

            impl<Stub> ::core::convert::From<Stub> for #client_ident<Stub>
                where Stub: ::tarpc::client::stub::Stub<
                    Req = #request_ident,
                    Resp = #response_ident>
            {
                /// Returns a new client stub that sends requests over the given transport.
                fn from(stub: Stub) -> Self {
                    #client_ident(stub)
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
            impl<Stub> #client_ident<Stub>
                where Stub: ::tarpc::client::stub::Stub<
                    Req = #request_ident,
                    Resp = #response_ident>
            {
                #(
                    #[allow(unused)]
                    #( #method_attrs )*
                    #vis fn #method_idents(&self, ctx: ::tarpc::context::Context, #( #args ),*)
                        -> impl ::core::future::Future<Output = ::core::result::Result<#return_types, ::tarpc::client::RpcError>> + '_ {
                        let request = #request_ident::#camel_case_idents { #( #arg_pats ),* };
                        let resp = self.0.call(ctx, request);
                        async move {
                            match resp.await? {
                                #response_ident::#camel_case_idents(msg) => ::core::result::Result::Ok(msg),
                                _ => ::core::unreachable!(),
                            }
                        }
                    }
                )*
            }
        }
    }

    fn emit_warnings(&self) -> TokenStream2 {
        self.warnings.iter().map(|w| w.to_token_stream()).collect()
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
            self.struct_client(),
            self.impl_client_new(),
            self.impl_client_rpc_methods(),
            self.emit_warnings(),
        ]);
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
