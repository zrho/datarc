use proc_macro2::TokenStream;
use quote::quote;
use syn::{Generics, Ident, Lifetime};

#[derive(Clone, Copy, Default)]
pub(super) struct EqualityOptions {
    pub ptr_eq: bool,
    pub hash: bool,
}

/// Forward through borrowed views, keeping each trait conditional independently.
pub(super) fn expand(
    owner: &Ident,
    generics: &Generics,
    view: &TokenStream,
    borrow: &Lifetime,
    equality: EqualityOptions,
    with_views: impl Fn(bool, TokenStream) -> TokenStream,
) -> TokenStream {
    let traits = [
        (
            quote!(::std::fmt::Debug),
            false,
            vec![(
                false,
                quote!(fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result),
                quote!(::std::fmt::Debug::fmt(__view, f)),
            )],
        ),
        (quote!(::std::cmp::Eq), false, vec![]),
        (
            quote!(::std::cmp::PartialOrd),
            true,
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
            false,
            vec![(
                true,
                quote!(fn cmp(&self, other: &Self) -> ::std::cmp::Ordering),
                quote!(::std::cmp::Ord::cmp(__view, __other)),
            )],
        ),
    ];
    let implementations = traits
        .into_iter()
        .map(|(trait_path, needs_equality, methods)| {
            let mut generics = generics.clone();
            // The higher-ranked bound also defers checking for concrete views that
            // do not implement the trait (including views with no borrow lifetime).
            generics
                .make_where_clause()
                .predicates
                .push(syn::parse_quote!(for<#borrow> #view: #trait_path));
            if equality.ptr_eq && needs_equality {
                generics
                    .make_where_clause()
                    .predicates
                    .push(syn::parse_quote!(for<#borrow> #view: ::std::cmp::Eq));
            }
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
    let mut equality_generics = generics.clone();
    let equality_trait = if equality.ptr_eq {
        quote!(::std::cmp::Eq)
    } else {
        quote!(::std::cmp::PartialEq)
    };
    equality_generics
        .make_where_clause()
        .predicates
        .push(syn::parse_quote!(for<#borrow> #view: #equality_trait));
    let (imp, args, bounds) = equality_generics.split_for_impl();
    let equality_methods =
        [(quote!(eq), true), (quote!(ne), false)]
            .into_iter()
            .map(|(method, equal)| {
                let unequal = !equal;
                let pointer = equality.ptr_eq.then(|| {
                    quote! {
                        if self.ptr_eq(other) { return #equal; }
                    }
                });
                let hash = equality.hash.then(|| {
                    quote! {
                        if self.cached_hash() != other.cached_hash() { return #unequal; }
                    }
                });
                let body = with_views(
                    true,
                    quote!(::std::cmp::PartialEq::#method(__view, __other)),
                );
                quote! {
                    #[inline]
                    fn #method(&self, other: &Self) -> bool {
                        #pointer
                        #hash
                        #body
                    }
                }
            });
    quote! {
        #(#implementations)*
        impl #imp ::std::cmp::PartialEq for #owner #args #bounds {
            #(#equality_methods)*
        }
    }
}
