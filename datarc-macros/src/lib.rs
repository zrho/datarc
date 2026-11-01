//! Attribute expansion for datarc's inline records.
#![deny(unsafe_op_in_unsafe_fn)]

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::{format_ident, quote};
use syn::ext::IdentExt;
use syn::{parse_macro_input, spanned::Spanned, Fields, Index, ItemStruct, Type};

/// Replace a named-field struct with a pointer-sized atomic shared handle.
#[proc_macro_attribute]
pub fn datarc(args: TokenStream, item: TokenStream) -> TokenStream {
    let item = parse_macro_input!(item as ItemStruct);
    expand(args.into(), item)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

struct Field {
    ident: syn::Ident,
    ty: Type,
    vis: syn::Visibility,
    attrs: Vec<syn::Attribute>,
    length: Option<Type>,
    slot: Index,
}

fn field_options(field: &syn::Field) -> syn::Result<(Type, Option<Type>)> {
    // Parentheses and invisible groups do not change whether a type is a slice.
    let mut declared_type = &field.ty;
    loop {
        declared_type = match declared_type {
            Type::Paren(ty) => &ty.elem,
            Type::Group(ty) => &ty.elem,
            _ => break,
        };
    }
    let slice_element = match declared_type {
        Type::Slice(ty) => Some(ty.elem.as_ref()),
        _ => None,
    };
    let mut multiplicity = None;
    let mut length = None;
    for attr in &field.attrs {
        if !attr.path().is_ident("datarc") {
            check_attr(attr)?;
            continue;
        }
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("many") || meta.path.is_ident("once") {
                if multiplicity.is_some() {
                    return Err(meta.error("specify multiplicity only once"));
                }
                multiplicity = Some(meta.path.is_ident("many"));
            } else if meta.path.is_ident("len") {
                if length.is_some() {
                    return Err(meta.error("specify the length type only once"));
                }
                let ty: Type = meta.value()?.parse()?;
                let valid = match &ty {
                    Type::Path(path) => {
                        path.qself.is_none()
                            && ["u8", "u16", "u32", "u64", "u128", "usize"]
                                .iter()
                                .any(|name| path.path.is_ident(name))
                    }
                    _ => false,
                };
                if !valid {
                    return Err(syn::Error::new(
                        ty.span(),
                        "length type must be u8, u16, u32, u64, u128, or usize",
                    ));
                }
                length = Some(ty);
            } else {
                return Err(
                    meta.error("expected `once`, `many`, or `len = <unsigned integer type>`")
                );
            }
            Ok(())
        })?;
    }
    if slice_element.is_some() && multiplicity == Some(false) {
        return Err(syn::Error::new(
            field.ty.span(),
            "a slice field is implicitly `many` and cannot be `once`",
        ));
    }
    let ty = slice_element.unwrap_or(&field.ty).clone();
    if slice_element.is_some() || multiplicity == Some(true) {
        Ok((ty, Some(length.unwrap_or_else(|| syn::parse_quote!(usize)))))
    } else if length.is_some() {
        Err(syn::Error::new(
            field.span(),
            "`len` requires a slice field `[T]` or explicit `many`",
        ))
    } else {
        Ok((ty, None))
    }
}

fn check_attr(attr: &syn::Attribute) -> syn::Result<()> {
    if [
        "doc",
        "allow",
        "warn",
        "deny",
        "forbid",
        "deprecated",
        "expect",
    ]
    .iter()
    .any(|name| attr.path().is_ident(name))
    {
        Ok(())
    } else {
        Err(syn::Error::new(attr.span(), "this attribute is not supported on datarc records or fields; Clone is generated automatically"))
    }
}

fn expand(args: TokenStream2, item: ItemStruct) -> syn::Result<TokenStream2> {
    if !args.is_empty() {
        return Err(syn::Error::new_spanned(
            args,
            "the struct attribute takes no arguments; configure lengths on individual fields",
        ));
    }
    for attr in &item.attrs {
        check_attr(attr)?;
    }
    // `Self` would change its meaning when transferred to the private header.
    struct FindSelf(Option<proc_macro2::Span>);
    impl<'ast> syn::visit::Visit<'ast> for FindSelf {
        fn visit_ident(&mut self, ident: &'ast syn::Ident) {
            if ident == "Self" {
                self.0 = Some(ident.span());
            }
        }
    }
    let mut find_self = FindSelf(None);
    syn::visit::Visit::visit_item_struct(&mut find_self, &item);
    if let Some(span) = find_self.0 {
        return Err(syn::Error::new(
            span,
            "use the record's name instead of `Self` in its declaration",
        ));
    }
    let Fields::Named(named) = &item.fields else {
        return Err(syn::Error::new(
            item.fields.span(),
            "datarc requires a struct with named fields",
        ));
    };
    let mut once_count = 0;
    let mut many_count = 0;
    let mut fields = Vec::new();
    for field in &named.named {
        let ident = field.ident.clone().expect("named field");
        if ident.unraw() == "new" || ident.unraw() == "try_new" {
            return Err(syn::Error::new(
                ident.span(),
                "field name conflicts with a generated constructor",
            ));
        }
        let (ty, length) = field_options(field)?;
        let count = if length.is_some() {
            &mut many_count
        } else {
            &mut once_count
        };
        let slot = Index::from(*count);
        *count += 1;
        fields.push(Field {
            ident,
            ty,
            vis: field.vis.clone(),
            attrs: field
                .attrs
                .iter()
                .filter(|a| !a.path().is_ident("datarc"))
                .cloned()
                .collect(),
            length,
            slot,
        });
    }

    let runtime = match proc_macro_crate::crate_name("datarc") {
        Ok(proc_macro_crate::FoundCrate::Name(name)) => {
            let ident = format_ident!("{name}");
            quote!(::#ident)
        }
        // `::datarc` also resolves in integration tests of the runtime package.
        _ => quote!(::datarc),
    };
    let name = &item.ident;
    let storage = format_ident!("__DatarcStorage_{}", name);
    let attrs = &item.attrs;
    let vis = &item.vis;
    let generics = &item.generics;
    let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();
    let once: Vec<_> = fields.iter().filter(|f| f.length.is_none()).collect();
    let many: Vec<_> = fields.iter().filter(|f| f.length.is_some()).collect();
    let once_types: Vec<_> = once.iter().map(|f| &f.ty).collect();
    let many_types: Vec<_> = many.iter().map(|f| &f.ty).collect();
    let length_types: Vec<_> = many.iter().map(|f| f.length.as_ref().unwrap()).collect();
    let many_indices: Vec<_> = many.iter().map(|f| &f.slot).collect();
    let inputs: Vec<_> = (0..fields.len())
        .map(|i| format_ident!("__datarc_input_{i}"))
        .collect();
    let names: Vec<_> = fields.iter().map(|f| &f.ident).collect();
    let param_types: Vec<_> = fields
        .iter()
        .map(|f| {
            let ty = &f.ty;
            if f.length.is_some() {
                quote!(impl ::std::iter::IntoIterator<Item = #ty, IntoIter: ::std::iter::ExactSizeIterator>)
            } else {
                quote!(#ty)
            }
        })
        .collect();
    let once_inputs: Vec<_> = fields
        .iter()
        .zip(&inputs)
        .filter(|(f, _)| f.length.is_none())
        .map(|(_, i)| i)
        .collect();
    let many_inputs: Vec<_> = fields
        .iter()
        .zip(&inputs)
        .filter(|(f, _)| f.length.is_some())
        .map(|(_, i)| i)
        .collect();
    let length_names: Vec<_> = many.iter().map(|f| &f.ident).collect();

    let storage_expr = quote!(<#storage #ty_generics>);
    let accessors = fields.iter().map(|f| {
        let Field {
            ident,
            ty,
            vis,
            attrs,
            length,
            slot,
        } = f;
        if length.is_none() {
            quote! {
                #(#attrs)*
                #[inline]
                #vis fn #ident(&self) -> &#ty { &self.__datarc.get().__values.#slot }
            }
        } else {
            let preceding_arrays = many.iter().take(slot.index as usize).map(|preceding| {
                let preceding_ty = &preceding.ty;
                let preceding_slot = &preceding.slot;
                quote! {
                    let __align = ::std::mem::align_of::<#preceding_ty>();
                    let __offset = __offset.wrapping_add(__align - 1) & !(__align - 1);
                    let __offset = __offset.wrapping_add(
                        (__header.__lengths.#preceding_slot as usize)
                            .wrapping_mul(::std::mem::size_of::<#preceding_ty>())
                    );
                }
            });
            quote! {
                #(#attrs)*
                #[inline]
                #vis fn #ident(&self) -> &[#ty] {
                    let __header = self.__datarc.get();
                    // Construction validated every size and alignment operation,
                    // and lengths are immutable. These operations cannot wrap
                    // for a live record; wrapping arithmetic avoids rechecking
                    // those invariants even with overflow checks enabled.
                    let __offset = ::std::mem::size_of::<#storage #ty_generics>();
                    #(#preceding_arrays)*
                    let __align = ::std::mem::align_of::<#ty>();
                    let __offset = __offset.wrapping_add(__align - 1) & !(__align - 1);
                    // SAFETY: the stored length and offset describe initialized,
                    // aligned elements, kept alive by this borrowed strong owner.
                    unsafe {
                        ::std::slice::from_raw_parts(
                            self.__datarc.as_ptr().cast::<u8>().add(__offset).cast::<#ty>(),
                            __header.__lengths.#slot as usize,
                        )
                    }
                }
            }
        }
    });
    let accessors = accessors.collect::<Vec<_>>();
    let offset_names: Vec<_> = (0..many_count)
        .map(|i| format_ident!("__datarc_offset_{i}"))
        .collect();
    let initialize_tails = if many_count == 0 {
        TokenStream2::new()
    } else {
        quote! {
            // SAFETY: each guard owns a distinct aligned array region from
            // the checked layout. The allocation guard outlives these guards.
            let mut __tails = unsafe { (#(
                #runtime::__private::TailInit::<#many_types>::new(
                    __allocation.as_ptr().add(__offsets[#many_indices]).cast(),
                    __reported_lengths[#many_indices],
                ),
            )*) };
            #(__tails.#many_indices.fill(#many_inputs, stringify!(#length_names))?;)*
            ::std::mem::forget(__tails);
        }
    };
    Ok(quote! {
        #(#attrs)*
        #[repr(transparent)]
        #vis struct #name #generics #where_clause {
            __datarc: #runtime::__private::Shared<#storage #ty_generics>,
        }

        #[doc(hidden)]
        #[allow(non_camel_case_types)]
        #[repr(C)]
        struct #storage #generics #where_clause {
            __count: #runtime::__private::RefCount,
            __lengths: (#(#length_types,)*),
            __values: (#(#once_types,)*),
            __marker: ::std::marker::PhantomData<(#(#many_types,)*)>,
        }

        impl #impl_generics #storage #ty_generics #where_clause {
            fn __datarc_layout(__lengths: [usize; #many_count])
                -> ::std::result::Result<(::std::alloc::Layout, [usize; #many_count]), #runtime::BuildError>
            {
                let __layout = ::std::alloc::Layout::new::<Self>();
                #(
                    let __tail = ::std::alloc::Layout::array::<#many_types>(__lengths[#many_indices])
                        .map_err(|_| #runtime::BuildError::LayoutOverflow)?;
                    let (__layout, #offset_names) = __layout.extend(__tail)
                        .map_err(|_| #runtime::BuildError::LayoutOverflow)?;
                )*
                ::std::result::Result::Ok((__layout.pad_to_align(), [#(#offset_names,)*]))
            }
        }

        // SAFETY: the header owns the count, fixed values, and immutable lengths;
        // PhantomData includes all tail types in Send/Sync and drop checking.
        unsafe impl #impl_generics #runtime::__private::Storage for #storage #ty_generics #where_clause {
            fn ref_count(&self) -> &#runtime::__private::RefCount { &self.__count }
            fn layout(&self) -> ::std::alloc::Layout {
                Self::__datarc_layout([#(self.__lengths.#many_indices as usize,)*])
                    .expect("validated datarc layout").0
            }
            unsafe fn drop_tail(__ptr: *mut Self) {
                // SAFETY: caller is the last strong owner of initialized storage.
                unsafe {
                    let __lengths = [#((*__ptr).__lengths.#many_indices as usize,)*];
                    let (_, __offsets) = Self::__datarc_layout(__lengths).expect("validated datarc layout");
                    // Tuple drop glue destroys later tails even if an earlier
                    // element's destructor unwinds.
                    let __guards = (#(
                        #runtime::__private::TailDrop::new(::std::ptr::slice_from_raw_parts_mut(
                            __ptr.cast::<u8>().add(__offsets[#many_indices]).cast::<#many_types>(),
                            __lengths[#many_indices],
                        )),
                    )*);
                    ::std::mem::drop(__guards);
                }
            }
        }

        impl #impl_generics #name #ty_generics #where_clause {
            /// Construct a record from exact-size iterable inputs.
            /// Panics if validation fails or an input iterator panics.
            #vis fn new(#(#names: #param_types),*) -> Self {
                match Self::try_new(#(#names),*) {
                    ::std::result::Result::Ok(__value) => __value,
                    ::std::result::Result::Err(__error) => ::std::panic!("{}", __error),
                }
            }

            /// Construct a record from exact-size iterable inputs, validating
            /// reported lengths, allocation layout, and actual item counts.
            /// Iterator panics propagate after cleaning up initialized values.
            /// Allocation failure follows the standard allocator's failure behavior.
            #vis fn try_new(#(#names: #param_types),*) -> ::std::result::Result<Self, #runtime::BuildError> {
                let (#(#inputs,)*) = (#(#names,)*);
                #(let #many_inputs = ::std::iter::IntoIterator::into_iter(#many_inputs);)*
                let __reported_lengths = [#(::std::iter::ExactSizeIterator::len(&#many_inputs),)*];
                let __lengths: (#(#length_types,)*) = (#(
                    <#length_types as ::std::convert::TryFrom<usize>>::try_from(__reported_lengths[#many_indices])
                        .map_err(|_| #runtime::BuildError::LengthOverflow {
                            field: stringify!(#length_names), len: __reported_lengths[#many_indices],
                            length_type: stringify!(#length_types),
                        })?,
                )*);
                let (__layout, __offsets) = #storage_expr ::__datarc_layout(__reported_lengths)?;
                let __allocation = #runtime::__private::allocate(__layout);
                // SAFETY: header initialization cannot unwind. Install its
                // allocation guard before calling any user iterator code.
                unsafe {
                    __allocation.cast::<#storage #ty_generics>().as_ptr().write(#storage {
                        __count: #runtime::__private::RefCount::default(),
                        __lengths,
                        __values: (#(#once_inputs,)*),
                        __marker: ::std::marker::PhantomData,
                    });
                }
                // SAFETY: the header is initialized and uniquely owned here.
                let __header_guard = unsafe {
                    #runtime::__private::Allocation::new(__allocation.cast::<#storage #ty_generics>(), __layout)
                };
                #initialize_tails
                // Every array is fully initialized and every input iterator has
                // been destroyed. Transfer all destruction rights to Shared;
                // no user code can unwind between disarming guards and transfer.
                let __ptr = __header_guard.into_raw();
                // SAFETY: this is a complete allocation with count one and no
                // remaining initialization guards or other owners.
                ::std::result::Result::Ok(Self {
                    __datarc: unsafe { #runtime::__private::Shared::from_raw(__ptr) }
                })
            }

            #(#accessors)*
        }

        impl #impl_generics ::std::clone::Clone for #name #ty_generics #where_clause {
            fn clone(&self) -> Self { Self { __datarc: self.__datarc.clone() } }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn error(source: &str) -> String {
        expand(TokenStream2::new(), syn::parse_str(source).unwrap())
            .unwrap_err()
            .to_string()
    }

    #[test]
    fn rejects_unsupported_shapes() {
        for source in ["struct Tuple(u8);", "struct Unit;"] {
            assert!(error(source).contains("named fields"));
        }
    }

    #[test]
    fn rejects_invalid_field_configuration() {
        for (source, message) in [
            (
                "struct R { #[datarc(many, len = i16)] items: u8 }",
                "length type must be",
            ),
            (
                "struct R { #[datarc(len = u8)] items: u8 }",
                "requires a slice field",
            ),
            (
                "struct R { #[datarc(once)] items: [u8] }",
                "cannot be `once`",
            ),
            (
                "struct R { #[datarc(len = i16)] items: [u8] }",
                "length type must be",
            ),
            (
                "struct R { #[datarc(many, once)] items: u8 }",
                "multiplicity only once",
            ),
            (
                "struct R { #[datarc(many)] #[datarc(many)] items: u8 }",
                "multiplicity only once",
            ),
            (
                "struct R { #[datarc(many, len = u8, len = u16)] items: u8 }",
                "length type only once",
            ),
            (
                "struct R { #[datarc(unknown)] items: u8 }",
                "expected `once`",
            ),
            (
                "struct R { #[cfg(unix)] items: u8 }",
                "attribute is not supported",
            ),
            ("struct R { new: u8 }", "conflicts"),
            ("struct R { r#new: u8 }", "conflicts"),
            ("struct R { try_new: u8 }", "conflicts"),
            ("struct R { items: Box<Self> }", "instead of `Self`"),
            (
                "#[derive(Clone)] struct R { items: u8 }",
                "Clone is generated",
            ),
            (
                "#[repr(C)] struct R { items: u8 }",
                "attribute is not supported",
            ),
        ] {
            assert!(error(source).contains(message), "{source}");
        }
        let item = syn::parse_quote!(
            struct R {
                items: u8,
            }
        );
        assert!(expand(quote!(count = u32), item)
            .unwrap_err()
            .to_string()
            .contains("takes no arguments"));
    }

    #[test]
    fn generated_items_parse_with_generics_and_empty_records() {
        for source in [
            "struct Empty {}",
            "struct R<'a, T: 'a, const N: usize = 2> where T: Send { label: &'a str, #[datarc(many, len = u8)] items: T, fixed: [u8; N] }",
            "struct R<'a, T: 'a, const N: usize = 2> { label: &'a str, #[datarc(len = u8)] items: [T], fixed: [u8; N], nested: [[u8; N]], borrowed: &'a [T] }",
        ] {
            let tokens = expand(TokenStream2::new(), syn::parse_str(source).unwrap()).unwrap();
            syn::parse2::<syn::File>(tokens).unwrap();
        }
    }
}
