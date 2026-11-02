use proc_macro2::TokenStream;
use quote::quote;
use syn::{Generics, Ident, Lifetime};

/// Forward through borrowed views, keeping each trait conditional independently.
pub(super) fn expand(
    owner: &Ident,
    generics: &Generics,
    view: &TokenStream,
    borrow: &Lifetime,
    with_views: impl Fn(bool, TokenStream) -> TokenStream,
) -> TokenStream {
    let traits = [
        (
            quote!(::std::fmt::Debug),
            vec![(
                false,
                quote!(fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result),
                quote!(::std::fmt::Debug::fmt(__view, f)),
            )],
        ),
        (
            quote!(::std::cmp::PartialEq),
            vec![
                (
                    true,
                    quote!(fn eq(&self, other: &Self) -> bool),
                    quote!(::std::cmp::PartialEq::eq(__view, __other)),
                ),
                (
                    true,
                    quote!(fn ne(&self, other: &Self) -> bool),
                    quote!(::std::cmp::PartialEq::ne(__view, __other)),
                ),
            ],
        ),
        (quote!(::std::cmp::Eq), vec![]),
        (
            quote!(::std::cmp::PartialOrd),
            vec![
                (
                    true,
                    quote!(fn partial_cmp(&self, other: &Self) -> ::std::option::Option<::std::cmp::Ordering>),
                    quote!(::std::cmp::PartialOrd::partial_cmp(__view, __other)),
                ),
                (
                    true,
                    quote!(fn lt(&self, other: &Self) -> bool),
                    quote!(::std::cmp::PartialOrd::lt(__view, __other)),
                ),
                (
                    true,
                    quote!(fn le(&self, other: &Self) -> bool),
                    quote!(::std::cmp::PartialOrd::le(__view, __other)),
                ),
                (
                    true,
                    quote!(fn gt(&self, other: &Self) -> bool),
                    quote!(::std::cmp::PartialOrd::gt(__view, __other)),
                ),
                (
                    true,
                    quote!(fn ge(&self, other: &Self) -> bool),
                    quote!(::std::cmp::PartialOrd::ge(__view, __other)),
                ),
            ],
        ),
        (
            quote!(::std::cmp::Ord),
            vec![(
                true,
                quote!(fn cmp(&self, other: &Self) -> ::std::cmp::Ordering),
                quote!(::std::cmp::Ord::cmp(__view, __other)),
            )],
        ),
    ];
    let implementations = traits.into_iter().map(|(trait_path, methods)| {
        let mut generics = generics.clone();
        // The higher-ranked bound also defers checking for concrete views that
        // do not implement the trait (including views with no borrow lifetime).
        generics
            .make_where_clause()
            .predicates
            .push(syn::parse_quote!(for<#borrow> #view: #trait_path));
        let (imp, args, bounds) = generics.split_for_impl();
        let methods = methods.into_iter().map(|(binary, signature, call)| {
            let body = with_views(binary, call);
            quote!(#[inline] #signature { #body })
        });
        quote! {
            impl #imp #trait_path for #owner #args #bounds {
                #(#methods)*
            }
        }
    });
    quote!(#(#implementations)*)
}
