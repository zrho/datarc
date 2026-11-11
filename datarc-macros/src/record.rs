use proc_macro2::TokenStream;
use quote::quote;
use syn::{punctuated::Punctuated, spanned::Spanned, Fields, ItemStruct, Path, Token, Type};

pub(super) fn expand(args: TokenStream, mut item: ItemStruct) -> syn::Result<TokenStream> {
    let mut hash = false;
    let mut hasher = None;
    let mut equality = super::traits::EqualityOptions::default();
    let mut derives = Vec::new();
    let mut attrs = Vec::new();
    // Attribute arguments and helper attributes accept the same record options.
    let options: syn::Attribute = syn::parse_quote!(#[datarc(#args)]);
    let mut option_attrs = Vec::new();
    if !args.is_empty() {
        option_attrs.push(options);
    }
    for attr in &item.attrs {
        if attr.path().is_ident("datarc") {
            option_attrs.push(attr.clone());
        } else if attr.path().is_ident("derive") {
            for path in attr.parse_args_with(Punctuated::<Path, Token![,]>::parse_terminated)? {
                let name = path.segments.last().unwrap().ident.to_string();
                if ![
                    "Clone",
                    "Debug",
                    "PartialEq",
                    "Eq",
                    "PartialOrd",
                    "Ord",
                    "Hash",
                ]
                .contains(&name.as_str())
                    || (path.segments.len() > 1
                        && !["std", "core"]
                            .iter()
                            .any(|root| path.segments[0].ident == *root))
                {
                    return Err(syn::Error::new(path.span(), "unsupported record derive; supported traits are Clone, Debug, PartialEq, Eq, PartialOrd, Ord, and Hash"));
                }
                if derives.contains(&name) {
                    return Err(syn::Error::new(
                        path.span(),
                        "specify each derive only once",
                    ));
                }
                derives.push(name);
            }
        } else {
            super::check_attr(attr)?;
            attrs.push(attr.clone());
        }
    }
    for attr in option_attrs {
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("eq") {
                return meta.parse_nested_meta(|option| {
                    let enabled = if option.path.is_ident("ptr_eq") {
                        &mut equality.ptr_eq
                    } else if option.path.is_ident("hash") {
                        &mut equality.hash
                    } else {
                        return Err(option.error("expected `ptr_eq` or `hash`"));
                    };
                    if *enabled {
                        return Err(option.error("specify each equality shortcut only once"));
                    }
                    *enabled = true;
                    Ok(())
                });
            }
            if meta.path.is_ident("hash") {
                if hash {
                    return Err(meta.error("specify `hash` only once"));
                }
                hash = true;
                return Ok(());
            }
            if meta.path.is_ident("hasher") {
                if hasher.is_some() {
                    return Err(meta.error("specify the hasher only once"));
                }
                hasher = Some(meta.value()?.parse::<Type>()?);
                return Ok(());
            }
            Err(meta.error("expected `hash`, `hasher = Type`, or `eq(...)`"))
        })?;
    }
    if hasher.is_some() && !hash {
        return Err(syn::Error::new(
            item.ident.span(),
            "`hasher` requires `hash`",
        ));
    }
    if equality.hash && !hash {
        return Err(syn::Error::new(
            item.ident.span(),
            "`eq(hash)` requires `hash`",
        ));
    }
    if (equality.ptr_eq || equality.hash) && !derives.iter().any(|d| d == "PartialEq") {
        return Err(syn::Error::new(
            item.ident.span(),
            "equality shortcuts require deriving PartialEq",
        ));
    }
    item.attrs = attrs;
    let Fields::Named(fields) = &item.fields else {
        return Err(syn::Error::new(
            item.fields.span(),
            "record requires a struct with named fields",
        ));
    };
    let names: Vec<_> = fields
        .named
        .iter()
        .map(|f| f.ident.as_ref().unwrap())
        .collect();
    let types: Vec<Type> = fields
        .named
        .iter()
        .map(|f| {
            let (ty, length, mutable, string) = super::field_options(f)?;
            if hash && mutable {
                return Err(syn::Error::new(
                    f.span(),
                    "mutable fields cannot be combined with cached hashing",
                ));
            }
            Ok(if string {
                syn::parse_quote!(str)
            } else if length.is_some() {
                syn::parse_quote!([#ty])
            } else {
                ty
            })
        })
        .collect::<syn::Result<_>>()?;
    let hash_config = if hash {
        let hasher = hasher
            .unwrap_or_else(|| syn::parse_quote!(::std::collections::hash_map::DefaultHasher));
        let bound = quote!(#(#types: ::std::hash::Hash,)*);
        let compute = quote! {
            let mut __hasher = <#hasher as ::std::default::Default>::default();
            #(::std::hash::Hash::hash(__owner.#names(), &mut __hasher);)*
            let __hash = ::std::hash::Hasher::finish(&__hasher);
            // SAFETY: this owner is not published and the hash contains no references.
            unsafe { (*__owner.__datarc.as_ptr()).__hash = __hash; }
        };
        Some((bound, compute))
    } else {
        None
    };
    let trait_impls = super::traits::record_traits(&item, &types, &derives, equality, hash);
    let owner = super::generate_owner(item, hash_config)?;
    Ok(quote!(#owner #trait_impls))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Check invalid options, field shapes, and reserved method names.
    #[test]
    fn rejects_invalid_records_with_specific_diagnostics() {
        for (source, expected) in [
            (
                "#[derive(PartialEq)] #[datarc(eq(hash))] struct Record { value: u32 }",
                "`eq(hash)` requires `hash`",
            ),
            (
                "#[datarc(eq(ptr_eq, ptr_eq))] struct Record { value: u32 }",
                "equality shortcut only once",
            ),
            (
                "#[datarc(hash, eq(hash), eq(hash))] struct Record { value: u32 }",
                "equality shortcut only once",
            ),
            (
                "#[datarc(eq(unknown))] struct Record { value: u32 }",
                "expected `ptr_eq` or `hash`",
            ),
            (
                "#[datarc(eq())] struct Record { value: u32 }",
                "expected nested attribute",
            ),
            (
                "#[datarc(eq)] struct Record { value: u32 }",
                "expected parentheses",
            ),
            (
                "#[datarc(eq(ptr_eq = true))] struct Record { value: u32 }",
                "expected `,`",
            ),
            ("struct Record { ptr_eq: u32 }", "conflicts"),
            ("struct Record { r#ptr_eq: u32 }", "conflicts"),
            (
                "#[datarc(hash, hash)] struct Record { value: u32 }",
                "only once",
            ),
            (
                "#[datarc(hasher = Foo)] struct Record { value: u32 }",
                "requires `hash`",
            ),
            (
                "#[datarc(hash, hasher = Foo, hasher = Bar)] struct Record { value: u32 }",
                "hasher only once",
            ),
            (
                "#[datarc(hash)] struct Record { cached_hash: u32 }",
                "conflicts",
            ),
            ("struct Record(u8);", "named fields"),
            ("struct Record;", "named fields"),
            (
                "struct Record { #[datarc(many)] value: u32 }",
                "expected `len",
            ),
            (
                "struct Record { #[datarc(once)] value: [u32] }",
                "expected `len",
            ),
            (
                "struct Record { #[datarc(len = i8)] value: [u32] }",
                "length type must",
            ),
            (
                "struct Record { #[datarc(len = u8)] value: u32 }",
                "`len` requires a sequence field",
            ),
            (
                "struct Record { #[datarc(len = u8, len = u16)] value: [u32] }",
                "length type only once",
            ),
            (
                "struct Record { #[datarc(unknown)] value: [u32] }",
                "expected `len",
            ),
            ("struct Record { new: u32 }", "conflicts"),
            ("struct Record { r#new: u32 }", "conflicts"),
            ("struct Record { try_new: u32 }", "conflicts"),
            ("struct Record { new_static: u32 }", "conflicts"),
            ("struct Record { value: Self }", "instead of `Self`"),
            (
                "#[datarc(unknown)] struct Record { value: u32 }",
                "expected `hash`",
            ),
        ] {
            let error = expand(TokenStream::new(), syn::parse_str(source).unwrap())
                .unwrap_err()
                .to_string();
            assert!(error.contains(expected), "{source}: {error}");
        }
    }

    /// Check expansion of empty records and payload lifetime, type, and const generics.
    #[test]
    fn generics_are_preserved_in_generated_items() {
        for source in [
            "struct Empty {}",
            "struct Record<'data, T, const N: usize = 2> where T: Clone { value: T, payload: &'data str, rows: [[u8; N]] }",
            "struct Record<'a: 'b, 'b> where 'a: 'b { first: &'a u32, second: &'b [u32] }",
        ] {
            let tokens = expand(TokenStream::new(), syn::parse_str(source).unwrap()).unwrap();
            syn::parse2::<syn::File>(tokens).unwrap();
        }
    }
}
