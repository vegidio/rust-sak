//! The expansion behind [`instrument`](crate::instrument).

use proc_macro2::{Ident, TokenStream};
use quote::quote;
use syn::parse::Parser;
use syn::punctuated::Punctuated;
use syn::{Error, Expr, ExprLit, FnArg, ItemFn, Lit, Meta, Pat, Signature, Token, parse2};

/// Rewrites `function` so its body runs inside a span, reporting malformed input as a compile error.
///
/// On failure the original item is emitted alongside the error, so the only diagnostic the user sees is the real one
/// rather than a cascade of "cannot find function" errors from every call site.
pub(crate) fn expand(args: TokenStream, item: TokenStream) -> TokenStream {
    match try_expand(args, item.clone()) {
        Ok(tokens) => tokens,
        Err(error) => {
            let error = error.to_compile_error();
            quote! { #error #item }
        }
    }
}

/// The fallible half of [`expand`].
fn try_expand(args: TokenStream, item: TokenStream) -> syn::Result<TokenStream> {
    let options = Options::parse(args)?;
    let mut function: ItemFn = parse2(item)?;

    let name = options.name.clone().unwrap_or_else(|| function.sig.ident.to_string());
    let fields = captured_fields(&function.sig, &options);
    let body = &function.block;

    // Expands to the `span!` macro rather than restating what it does. That keeps the gate, the field
    // representation and the `Span` construction in one place — the attribute would otherwise be a second
    // implementation of all three, in a different language, free to drift from the macro it mirrors.
    //
    // The `use` is what lets the argument capture below resolve; it is scoped to this block, so it cannot disturb
    // anything the instrumented function's own body imports.
    let open = quote! {
        let __o11y_span = {
            use ::rust_sak::o11y::{ValueViaDebug as _, ValueViaInto as _};
            ::rust_sak::o11y::trace::span!(#name #fields)
        };
    };

    // An `async fn` cannot hold the guard across an `.await`: the task may resume on a different thread, where the
    // stack the guard pushed onto does not exist. Wrapping the body in a future that enters the span around every
    // poll is the only shape that survives that, so the two cases expand differently.
    let instrumented = if function.sig.asyncness.is_some() {
        quote! {{
            #open
            ::rust_sak::o11y::trace::__instrument(__o11y_span, async move #body).await
        }}
    } else {
        quote! {{
            #open
            #body
        }}
    };

    function.block = parse2(instrumented)?;

    Ok(quote! { #function })
}

/// Builds the trailing `, key = value` arguments handed to `span!` for the captured parameters.
///
/// Empty when there is nothing to capture, which is exactly the no-field `span!(name)` form — so no special case is
/// needed here or in the macro.
///
/// Only plain `name: Type` parameters are captured. A `self` receiver carries no useful name, and a destructuring
/// pattern such as `(x, y): (u32, u32)` has no single one, so both are skipped rather than guessed at.
fn captured_fields(signature: &Signature, options: &Options) -> TokenStream {
    if options.skip_all {
        return TokenStream::new();
    }

    let entries: Vec<TokenStream> = signature
        .inputs
        .iter()
        .filter_map(|argument| {
            let FnArg::Typed(typed) = argument else { return None };
            let Pat::Ident(pattern) = &*typed.pat else { return None };

            let name = &pattern.ident;
            if options.skip.iter().any(|skipped| skipped == name) {
                return None;
            }

            // The parameter's own identifier is the field key: `span!` stringifies it, which gives the same name
            // the signature wrote.
            //
            // Captured by reference, so instrumenting a function never moves an argument out from under its body.
            // `o11y_value` picks the cheap conversion for anything that has one and falls back to `Debug` for the
            // rest — see `o11y::capture`.
            Some(quote! { #name = (&#name).o11y_value() })
        })
        .collect();

    quote! { #(, #entries)* }
}

/// The parsed `#[instrument(..)]` arguments.
#[derive(Default)]
struct Options {
    /// Overrides the span name. Defaults to the function's own name.
    name: Option<String>,
    /// Arguments left out of the captured fields.
    skip: Vec<Ident>,
    /// Whether to capture no arguments at all.
    skip_all: bool,
}

impl Options {
    /// Parses `name = "..."`, `skip(a, b)` and `skip_all`, in any order and any combination.
    fn parse(args: TokenStream) -> syn::Result<Self> {
        let mut options = Self::default();

        if args.is_empty() {
            return Ok(options);
        }

        for meta in Punctuated::<Meta, Token![,]>::parse_terminated.parse2(args)? {
            match &meta {
                Meta::NameValue(pair) if pair.path.is_ident("name") => {
                    let Expr::Lit(ExprLit {
                        lit: Lit::Str(text), ..
                    }) = &pair.value
                    else {
                        return Err(Error::new_spanned(&pair.value, "`name` expects a string literal"));
                    };

                    options.name = Some(text.value());
                }
                Meta::List(list) if list.path.is_ident("skip") => {
                    options
                        .skip
                        .extend(list.parse_args_with(Punctuated::<Ident, Token![,]>::parse_terminated)?);
                }
                Meta::Path(path) if path.is_ident("skip_all") => options.skip_all = true,
                other => {
                    return Err(Error::new_spanned(
                        other,
                        "expected `name = \"...\"`, `skip(..)` or `skip_all`",
                    ));
                }
            }
        }

        Ok(options)
    }
}
