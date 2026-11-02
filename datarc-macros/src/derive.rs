use std::collections::BTreeSet;

use proc_macro2::{Span, TokenStream};
use quote::{format_ident, quote};
use syn::{
    spanned::Spanned, Fields, GenericParam, Generics, ItemStruct, Lifetime, Type, TypeParamBound,
    WherePredicate,
};

fn ungroup(mut ty: &Type) -> &Type {
    loop {
        ty = match ty {
            Type::Paren(ty) => &ty.elem,
            Type::Group(ty) => &ty.elem,
            _ => return ty,
        };
    }
}

fn contains_borrow_lifetime<T>(
    value: &T,
    borrows: &BTreeSet<String>,
    visit: impl FnOnce(&mut FindLifetime<'_>, &T),
) -> bool {
    let mut finder = FindLifetime {
        borrows,
        found: false,
    };
    visit(&mut finder, value);
    finder.found
}

struct FindLifetime<'a> {
    borrows: &'a BTreeSet<String>,
    found: bool,
}

impl<'ast> syn::visit::Visit<'ast> for FindLifetime<'_> {
    fn visit_lifetime(&mut self, lifetime: &'ast Lifetime) {
        self.found |= self.borrows.contains(&lifetime.ident.to_string());
    }
}

fn owner_generics(original: &Generics, borrows: &BTreeSet<String>) -> syn::Result<Generics> {
    let mut result = original.clone();
    let unsupported = |span| {
        syn::Error::new(span, "view lifetimes may only appear on outer field references and in outlives bounds; use a separate lifetime for borrowed payloads")
    };
    let mut params = syn::punctuated::Punctuated::new();
    for mut param in result.params {
        match &mut param {
            GenericParam::Lifetime(lt) if borrows.contains(&lt.lifetime.ident.to_string()) => {
                if lt
                    .bounds
                    .iter()
                    .any(|b| !borrows.contains(&b.ident.to_string()))
                {
                    return Err(unsupported(lt.span()));
                }
                continue;
            }
            GenericParam::Lifetime(lt) => {
                lt.bounds = lt
                    .bounds
                    .iter()
                    .filter(|b| !borrows.contains(&b.ident.to_string()))
                    .cloned()
                    .collect();
            }
            GenericParam::Type(ty) => {
                ty.bounds = ty.bounds.iter().filter(|bound| !matches!(bound, TypeParamBound::Lifetime(lt) if borrows.contains(&lt.ident.to_string()))).cloned().collect();
                if ty.bounds.is_empty() {
                    ty.colon_token = None;
                }
            }
            _ => {}
        }
        if contains_borrow_lifetime(&param, borrows, |v, p| {
            syn::visit::Visit::visit_generic_param(v, p)
        }) {
            return Err(unsupported(param.span()));
        }
        params.push(param);
    }
    result.params = params;
    if result.params.is_empty() {
        result.lt_token = None;
        result.gt_token = None;
    }
    if let Some(where_clause) = &mut result.where_clause {
        let mut predicates = syn::punctuated::Punctuated::new();
        for mut predicate in where_clause.predicates.clone() {
            match &mut predicate {
                WherePredicate::Type(p) => {
                    p.bounds = p.bounds.iter().filter(|bound| !matches!(bound, TypeParamBound::Lifetime(lt) if borrows.contains(&lt.ident.to_string()))).cloned().collect();
                    if p.bounds.is_empty() {
                        continue;
                    }
                }
                WherePredicate::Lifetime(p) if borrows.contains(&p.lifetime.ident.to_string()) => {
                    if p.bounds
                        .iter()
                        .any(|b| !borrows.contains(&b.ident.to_string()))
                    {
                        return Err(unsupported(p.span()));
                    }
                    continue;
                }
                WherePredicate::Lifetime(p) => {
                    p.bounds = p
                        .bounds
                        .iter()
                        .filter(|b| !borrows.contains(&b.ident.to_string()))
                        .cloned()
                        .collect();
                    if p.bounds.is_empty() {
                        continue;
                    }
                }
                _ => {}
            }
            if contains_borrow_lifetime(&predicate, borrows, |v, p| {
                syn::visit::Visit::visit_where_predicate(v, p)
            }) {
                return Err(unsupported(predicate.span()));
            }
            predicates.push(predicate);
        }
        where_clause.predicates = predicates;
        if where_clause.predicates.is_empty() {
            result.where_clause = None;
        }
    }
    Ok(result)
}

pub(super) fn fresh_lifetime(generics: &Generics, stem: &str) -> Lifetime {
    for suffix in 0.. {
        let name = format!("{stem}_{suffix}");
        if !generics.lifetimes().any(|p| p.lifetime.ident == name) {
            return Lifetime::new(&format!("'{name}"), Span::call_site());
        }
    }
    unreachable!()
}

pub(super) fn expand(item: ItemStruct) -> syn::Result<TokenStream> {
    let runtime = super::runtime_path();
    let name = &item.ident;
    let mut owner_name = None;
    let mut hash = false;
    let mut hasher = None;
    let mut equality = super::traits::EqualityOptions::default();
    for attr in item.attrs.iter().filter(|a| a.path().is_ident("datarc")) {
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("eq") {
                meta.parse_nested_meta(|option| {
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
                })?;
                return Ok(());
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
            if !meta.path.is_ident("owned") {
                return Err(meta
                    .error("expected `owned = OwnerName`, `hash`, `hasher = Type`, or `eq(...)`"));
            }
            if owner_name.is_some() {
                return Err(meta.error("specify the owner name only once"));
            }
            owner_name = Some(meta.value()?.parse::<syn::Ident>()?);
            Ok(())
        })?;
    }
    if hasher.is_some() && !hash {
        return Err(syn::Error::new(name.span(), "`hasher` requires `hash`"));
    }
    if equality.hash && !hash {
        return Err(syn::Error::new(name.span(), "`eq(hash)` requires `hash`"));
    }
    let owner_name = owner_name.unwrap_or_else(|| format_ident!("{}Packed", name));
    if owner_name == *name {
        return Err(syn::Error::new(
            owner_name.span(),
            "owner and view must have different names",
        ));
    }
    let Fields::Named(original_fields) = &item.fields else {
        return Err(syn::Error::new(
            item.fields.span(),
            "Packable requires a struct with named reference fields",
        ));
    };
    let mut owner = item.clone();
    owner.ident = owner_name.clone();
    owner.attrs.retain(|a| super::check_attr(a).is_ok());
    let Fields::Named(owner_fields) = &mut owner.fields else {
        unreachable!()
    };
    let mut borrows = BTreeSet::new();
    let mut references = Vec::new();
    for (field, storage_field) in original_fields.named.iter().zip(&mut owner_fields.named) {
        let Type::Reference(reference) = ungroup(&field.ty) else {
            return Err(syn::Error::new(
                field.ty.span(),
                "Packable fields must be shared references: `&'a T` or `&'a [T]`",
            ));
        };
        if reference.mutability.is_some() {
            return Err(syn::Error::new(
                field.ty.span(),
                "Packable fields cannot be mutable references",
            ));
        }
        let Some(lifetime) = &reference.lifetime else {
            return Err(syn::Error::new(
                reference.span(),
                "field references must have an explicit view lifetime",
            ));
        };
        if lifetime.ident == "static" || lifetime.ident == "_" {
            return Err(syn::Error::new(
                lifetime.span(),
                "use a named generic lifetime for the view",
            ));
        }
        if !item
            .generics
            .lifetimes()
            .any(|p| p.lifetime.ident == lifetime.ident)
        {
            return Err(syn::Error::new(
                lifetime.span(),
                "view lifetime must be declared on the struct",
            ));
        }
        let element = ungroup(&reference.elem);
        if matches!(element, Type::TraitObject(_) | Type::ImplTrait(_))
            || matches!(element, Type::Path(p) if p.path.is_ident("str"))
        {
            return Err(syn::Error::new(element.span(), "referents must be sized values or slices with sized elements; use `&String` or `&[u8]` instead of `&str`"));
        }
        borrows.insert(lifetime.ident.to_string());
        references.push(reference.clone());
        storage_field.ty = reference.elem.as_ref().clone();
        storage_field
            .attrs
            .retain(|a| a.path().is_ident("datarc") || super::check_attr(a).is_ok());
        super::field_options(storage_field)?;
    }
    for reference in &references {
        if contains_borrow_lifetime(reference.elem.as_ref(), &borrows, |v, t| {
            syn::visit::Visit::visit_type(v, t)
        }) {
            return Err(syn::Error::new(reference.elem.span(), "use separate lifetimes for the outer view borrow and references stored inside its payload"));
        }
    }
    owner.generics = owner_generics(&item.generics, &borrows)?;
    let owner_generics = &owner.generics;
    let (_, owner_args, _) = owner_generics.split_for_impl();
    let (view_impl, view_args, view_where) = item.generics.split_for_impl();
    let borrow = fresh_lifetime(&item.generics, "__datarc_view");
    let view_ref = fresh_lifetime(&item.generics, "__datarc_ref");
    let borrowed_args: Vec<_> = item
        .generics
        .params
        .iter()
        .map(|p| match p {
            GenericParam::Lifetime(p) if borrows.contains(&p.lifetime.ident.to_string()) => {
                quote!(#borrow)
            }
            GenericParam::Lifetime(p) => {
                let lt = &p.lifetime;
                quote!(#lt)
            }
            GenericParam::Type(p) => {
                let id = &p.ident;
                quote!(#id)
            }
            GenericParam::Const(p) => {
                let id = &p.ident;
                quote!(#id)
            }
        })
        .collect();
    let borrowed_view = if borrowed_args.is_empty() {
        quote!(#name)
    } else {
        quote!(#name<#(#borrowed_args),*>)
    };
    let fields: Vec<_> = original_fields
        .named
        .iter()
        .map(|f| f.ident.as_ref().unwrap())
        .collect();
    let mut conversion_generics = item.generics.clone();
    let mut clone_values = Vec::new();
    for (field, reference) in original_fields.named.iter().zip(&references) {
        let ident = field.ident.as_ref().unwrap();
        let stored = ungroup(&reference.elem);
        if let Type::Slice(slice) = stored {
            let element = &slice.elem;
            conversion_generics.make_where_clause().predicates.push(
                syn::parse_quote!(#reference: #runtime::__private::CloneItems<Item = #element>),
            );
            clone_values.push(
                quote!(<#reference as #runtime::__private::CloneItems>::clone_items(__view.#ident)),
            );
        } else {
            conversion_generics.make_where_clause().predicates.push(
                syn::parse_quote!(#reference: #runtime::__private::CloneValue<Value = #stored>),
            );
            clone_values.push(
                quote!(<#reference as #runtime::__private::CloneValue>::clone_value(__view.#ident)),
            );
        }
    }
    let mut ref_generics = conversion_generics.clone();
    ref_generics.params.insert(
        0,
        syn::GenericParam::Lifetime(syn::LifetimeParam::new(view_ref.clone())),
    );
    let mut from_generics = owner.generics.clone();
    from_generics.params.insert(
        0,
        syn::GenericParam::Lifetime(syn::LifetimeParam::new(borrow.clone())),
    );
    from_generics
        .make_where_clause()
        .predicates
        .push(syn::parse_quote!(#owner_name #owner_args: #borrow));
    let (from_impl, _, from_where) = from_generics.split_for_impl();
    let inferred_args: Vec<_> = item.generics.params.iter().zip(&borrowed_args).map(|(param, arg)| {
        if matches!(param, GenericParam::Lifetime(p) if borrows.contains(&p.lifetime.ident.to_string())) {
            quote!('_)
        } else {
            arg.clone()
        }
    }).collect();
    let inferred_view = if inferred_args.is_empty() {
        quote!(#name)
    } else {
        quote!(#name<#(#inferred_args),*>)
    };
    let hash_config = if hash {
        let hasher = hasher
            .unwrap_or_else(|| syn::parse_quote!(::std::collections::hash_map::DefaultHasher));
        let bound = quote!(for<#borrow> #borrowed_view: ::std::hash::Hash);
        let compute = quote! {
            let __view: #inferred_view = ::std::convert::From::from(&*__owner);
            let mut __hasher = <#hasher as ::std::default::Default>::default();
            ::std::hash::Hash::hash(&__view, &mut __hasher);
            let __hash = ::std::hash::Hasher::finish(&__hasher);
            // SAFETY: this owner is not published and the hash contains no references.
            unsafe { (*__owner.__datarc.as_ptr()).__hash = __hash; }
        };
        Some((bound, compute))
    } else {
        None
    };
    if let Some((bound, _)) = &hash_config {
        let predicate: WherePredicate = syn::parse2(bound.clone())?;
        conversion_generics
            .make_where_clause()
            .predicates
            .push(predicate.clone());
        ref_generics.make_where_clause().predicates.push(predicate);
    }
    let (conversion_impl, _, conversion_where) = conversion_generics.split_for_impl();
    let (ref_impl, _, ref_where) = ref_generics.split_for_impl();
    let owner_code = super::generate_owner(owner.clone(), hash_config)?;
    let trait_impls = super::traits::expand(
        &owner_name,
        owner_generics,
        &borrowed_view,
        &borrow,
        equality,
        |binary, call| {
            let other = binary.then(|| {
                quote! {
                    let __other: &#inferred_view = &::std::convert::From::from(other);
                }
            });
            quote! {
                let __view: &#inferred_view = &::std::convert::From::from(self);
                #other
                #call
            }
        },
    );
    Ok(quote! {
        #owner_code
        #trait_impls

        impl #view_impl #runtime::Packable for #name #view_args #view_where {
            type Packed = #owner_name #owner_args;
        }

        impl #from_impl ::std::convert::From<&#borrow #owner_name #owner_args> for #borrowed_view #from_where {
            #[inline]
            fn from(__owner: &#borrow #owner_name #owner_args) -> Self {
                #name { #(#fields: __owner.#fields(),)* }
            }
        }

        impl #ref_impl ::std::convert::From<&#view_ref #name #view_args> for #owner_name #owner_args #ref_where {
            /// Clone the referenced contents. Panics on invalid lengths or layout.
            fn from(__view: &#view_ref #name #view_args) -> Self {
                Self::new(#(#clone_values),*)
            }
        }

        impl #conversion_impl ::std::convert::From<#name #view_args> for #owner_name #owner_args #conversion_where {
            /// Clone the referenced contents. Panics on invalid lengths or layout.
            fn from(__view: #name #view_args) -> Self {
                <Self as ::std::convert::From<&#name #view_args>>::from(&__view)
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Checks that invalid field types, lifetimes, attributes, and reserved names
    /// produce diagnostics identifying the violated rule.
    #[test]
    fn rejects_invalid_views_with_specific_diagnostics() {
        for (source, expected) in [
            (
                "#[datarc(eq(hash))] struct View<'a> { value: &'a u32 }",
                "`eq(hash)` requires `hash`",
            ),
            (
                "#[datarc(eq(ptr_eq, ptr_eq))] struct View<'a> { value: &'a u32 }",
                "equality shortcut only once",
            ),
            (
                "#[datarc(hash, eq(hash), eq(hash))] struct View<'a> { value: &'a u32 }",
                "equality shortcut only once",
            ),
            (
                "#[datarc(eq(unknown))] struct View<'a> { value: &'a u32 }",
                "expected `ptr_eq` or `hash`",
            ),
            (
                "#[datarc(eq())] struct View<'a> { value: &'a u32 }",
                "expected nested attribute",
            ),
            (
                "#[datarc(eq)] struct View<'a> { value: &'a u32 }",
                "expected parentheses",
            ),
            (
                "#[datarc(eq(ptr_eq = true))] struct View<'a> { value: &'a u32 }",
                "expected `,`",
            ),
            (
                "struct View<'a> { ptr_eq: &'a u32 }",
                "conflicts with the generated ptr_eq method",
            ),
            (
                "struct View<'a> { r#ptr_eq: &'a u32 }",
                "conflicts with the generated ptr_eq method",
            ),
            (
                "#[datarc(hash, hash)] struct View<'a> { value: &'a u32 }",
                "only once",
            ),
            (
                "#[datarc(hasher = Foo)] struct View<'a> { value: &'a u32 }",
                "requires `hash`",
            ),
            (
                "#[datarc(hash, hasher = Foo, hasher = Bar)] struct View<'a> { value: &'a u32 }",
                "hasher only once",
            ),
            (
                "#[datarc(hash)] struct View<'a> { cached_hash: &'a u32 }",
                "conflicts",
            ),
            ("struct View(u8);", "named reference fields"),
            ("struct View;", "named reference fields"),
            ("struct View { value: u32 }", "shared references"),
            (
                "struct View<'a> { value: &'a mut u32 }",
                "mutable references",
            ),
            ("struct View { value: &u32 }", "explicit view lifetime"),
            (
                "struct View { value: &'static u32 }",
                "named generic lifetime",
            ),
            (
                "struct View<'a> { value: &'a str }",
                "sized values or slices",
            ),
            (
                "struct View<'a> { #[datarc(many)] value: &'a u32 }",
                "expected `len",
            ),
            (
                "struct View<'a> { #[datarc(once)] value: &'a [u32] }",
                "expected `len",
            ),
            (
                "struct View<'a> { #[datarc(len = i8)] value: &'a [u32] }",
                "length type must",
            ),
            (
                "struct View<'a> { #[datarc(len = u8)] value: &'a u32 }",
                "`len` requires a slice field",
            ),
            (
                "struct View<'a> { #[datarc(len = u8, len = u16)] value: &'a [u32] }",
                "length type only once",
            ),
            (
                "struct View<'a> { #[datarc(unknown)] value: &'a [u32] }",
                "expected `len",
            ),
            (
                "struct View<'a> { value: &'a &'a str }",
                "separate lifetimes",
            ),
            (
                "struct View<'a, T: SomeTrait<'a>> { value: &'a T }",
                "view lifetimes may only",
            ),
            (
                "struct View<'a: 'static> { value: &'a u32 }",
                "view lifetimes may only",
            ),
            ("struct View<'a> { new: &'a u32 }", "conflicts"),
            ("struct View<'a> { r#new: &'a u32 }", "conflicts"),
            ("struct View<'a> { try_new: &'a u32 }", "conflicts"),
            ("struct View<'a> { new_static: &'a u32 }", "conflicts"),
            ("struct View<'a> { value: &'a Self }", "instead of `Self`"),
            (
                "#[datarc(owned = View)] struct View<'a> { value: &'a u32 }",
                "different names",
            ),
            (
                "#[datarc(owned = A, owned = B)] struct View<'a> { value: &'a u32 }",
                "owner name only once",
            ),
            (
                "#[datarc(unknown)] struct View<'a> { value: &'a u32 }",
                "expected `owned",
            ),
        ] {
            let error = expand(syn::parse_str(source).unwrap())
                .unwrap_err()
                .to_string();
            assert!(error.contains(expected), "{source}: {error}");
        }
    }

    /// Checks that expansion produces parseable Rust for empty views and views with
    /// payload lifetimes, outlives bounds, and type and const generics.
    #[test]
    fn lifetime_generics_are_filtered_and_rebound_in_generated_items() {
        for source in [
            "struct Empty {}",
            "#[datarc(owned = Owner)] struct View<'a, 'data: 'a, T: 'a, const N: usize = 2> where T: Clone + 'a { value: &'a T, payload: &'a &'data str, rows: &'a [[u8; N]] }",
            "struct View<'a: 'b, 'b> where 'a: 'b { first: &'a u32, second: &'b [u32] }",
        ] {
            let tokens = expand(syn::parse_str(source).unwrap()).unwrap();
            syn::parse2::<syn::File>(tokens).unwrap();
        }
    }
}
