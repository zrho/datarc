use proc_macro2::TokenStream;
use quote::quote;
use syn::{Fields, Generics, Ident, ItemStruct, Lifetime, Type};

pub(super) fn fresh_lifetime(generics: &Generics, stem: &str) -> Lifetime {
    for suffix in 0.. {
        let name = format!("{stem}_{suffix}");
        if !generics.lifetimes().any(|p| p.lifetime.ident == name) {
            return Lifetime::new(&format!("'{name}"), proc_macro2::Span::call_site());
        }
    }
    unreachable!()
}

#[derive(Clone, Copy, Default)]
pub(super) struct EqualityOptions {
    pub ptr_eq: bool,
    pub hash: bool,
}

pub(super) fn fresh_ident(generics: &Generics, stem: &str) -> Ident {
    let mut name = stem.to_owned();
    while generics.params.iter().any(|param| match param {
        syn::GenericParam::Type(p) => p.ident == name,
        syn::GenericParam::Const(p) => p.ident == name,
        _ => false,
    }) {
        name.push('_');
    }
    quote::format_ident!("{name}")
}

/// Recursive fields are checked by the implementation body. A bound such as
/// `[Node]: Debug` on `impl Debug for Node` would make trait resolution cyclic.
fn is_recursive(ty: &Type, owner: &Ident) -> bool {
    struct FindOwner<'a> {
        owner: &'a Ident,
        found: bool,
    }
    impl<'ast> syn::visit::Visit<'ast> for FindOwner<'_> {
        fn visit_type_path(&mut self, path: &'ast syn::TypePath) {
            self.found |= path.qself.is_none()
                && path
                    .path
                    .segments
                    .last()
                    .is_some_and(|s| s.ident == *self.owner);
            syn::visit::visit_type_path(self, path);
        }
    }
    let mut finder = FindOwner {
        owner,
        found: false,
    };
    syn::visit::Visit::visit_type(&mut finder, ty);
    finder.found
}

/// Generate requested standard traits from record fields, never from the pointer.
pub(super) fn record_traits(
    item: &ItemStruct,
    types: &[Type],
    derives: &[String],
    equality: EqualityOptions,
    cached_hash: bool,
) -> TokenStream {
    let owner = &item.ident;
    let Fields::Named(fields) = &item.fields else {
        unreachable!()
    };
    let names: Vec<_> = fields
        .named
        .iter()
        .map(|f| f.ident.as_ref().unwrap())
        .collect();
    let formatter = fresh_ident(&item.generics, "__datarc_formatter");
    let debug = fresh_ident(&item.generics, "__datarc_debug");
    let state = fresh_ident(&item.generics, "__datarc_state");
    let other = fresh_ident(&item.generics, "__datarc_other");
    let result = fresh_ident(&item.generics, "__datarc_result");
    let eq_type = fresh_ident(&item.generics, "__DatarcEqField");
    let require_eq = fresh_ident(&item.generics, "__datarc_require_eq");
    let assert_eq = fresh_ident(&item.generics, "__datarc_assert_eq");
    let eq_check = quote! {
        fn #require_eq<#eq_type: ?Sized + ::std::cmp::Eq>(_: &#eq_type) {}
        #(#require_eq(self.#names());)*
    };
    let implementations = derives.iter().filter_map(|name| {
        let (path, body) = match name.as_str() {
            "Clone" => return None, // Sharing is always available, without payload bounds.
            "Debug" => (quote!(::std::fmt::Debug), quote! {
                fn fmt(&self, #formatter: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
                    let mut #debug = #formatter.debug_struct(stringify!(#owner));
                    #(#debug.field(stringify!(#names), &self.#names());)*
                    #debug.finish()
                }
            }),
            "PartialEq" => {
                let pointer = equality.ptr_eq.then(|| quote! {
                    #eq_check
                    if self.ptr_eq(#other) { return true; }
                });
                let hash = equality.hash.then(|| quote!(if self.cached_hash() != #other.cached_hash() { return false; }));
                (quote!(::std::cmp::PartialEq), quote! {
                    fn eq(&self, #other: &Self) -> bool {
                        #pointer
                        #hash
                        true #(&& ::std::cmp::PartialEq::eq(self.#names(), #other.#names()))*
                    }
                })
            },
            "Eq" => (quote!(::std::cmp::Eq), quote!()),
            "PartialOrd" => (quote!(::std::cmp::PartialOrd), quote! {
                fn partial_cmp(&self, #other: &Self) -> ::std::option::Option<::std::cmp::Ordering> {
                    #(match ::std::cmp::PartialOrd::partial_cmp(self.#names(), #other.#names()) {
                        ::std::option::Option::Some(::std::cmp::Ordering::Equal) => {},
                        #result => return #result,
                    })*
                    ::std::option::Option::Some(::std::cmp::Ordering::Equal)
                }
            }),
            "Ord" => (quote!(::std::cmp::Ord), quote! {
                fn cmp(&self, #other: &Self) -> ::std::cmp::Ordering {
                    #(match ::std::cmp::Ord::cmp(self.#names(), #other.#names()) {
                        ::std::cmp::Ordering::Equal => {},
                        #result => return #result,
                    })*
                    ::std::cmp::Ordering::Equal
                }
            }),
            "Hash" => {
                let parameter = fresh_ident(&item.generics, "__DatarcHasher");
                let hash = if cached_hash {
                    quote!(::std::hash::Hasher::write_u64(#state, self.cached_hash());)
                } else {
                    quote!(#(::std::hash::Hash::hash(self.#names(), #state);)*)
                };
                (quote!(::std::hash::Hash), quote! {
                    fn hash<#parameter: ::std::hash::Hasher>(&self, #state: &mut #parameter) {
                        #hash
                    }
                })
            },
            _ => unreachable!(),
        };
        let mut generics = item.generics.clone();
        for ty in types {
            if !item.generics.params.is_empty() && !is_recursive(ty, owner) && (name != "Hash" || !cached_hash) {
                generics.make_where_clause().predicates.push(syn::parse_quote!(#ty: #path));
            }
            // Pointer equality must not make NaN or other non-reflexive values equal.
            // PartialOrd needs the same bound to satisfy its PartialEq supertrait.
            if !is_recursive(ty, owner) && equality.ptr_eq && ["PartialEq", "PartialOrd"].contains(&name.as_str()) {
                generics.make_where_clause().predicates.push(syn::parse_quote!(#ty: ::std::cmp::Eq));
            }
        }
        let (imp, args, bounds) = generics.split_for_impl();
        // Eq has no required methods, so check recursive fields in an unused
        // function body. This avoids circular where-bounds without accepting a
        // recursive wrapper whose PartialEq is not reflexive.
        let eq_assertion = (name == "Eq").then(|| quote! {
            const _: () = {
                fn #assert_eq #imp (#other: &#owner #args) #bounds {
                    fn #require_eq<#eq_type: ?Sized + ::std::cmp::Eq>(_: &#eq_type) {}
                    #(#require_eq(#other.#names());)*
                }
            };
        });
        Some(quote! {
            impl #imp #path for #owner #args #bounds { #body }
            #eq_assertion
        })
    });
    quote!(#(#implementations)*)
}

/// Forward through borrowed enums, keeping each trait conditional independently.
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
                // Enums can provide a custom ne; preserve it instead of using !eq.
                let ne_lint = (!equal).then(|| quote!(#[allow(clippy::partialeq_ne_impl)]));
                quote! {
                    #[inline]
                    #ne_lint
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
