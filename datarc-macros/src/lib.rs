//! Macro expansion for inline shared records and packed enums.
#![deny(unsafe_op_in_unsafe_fn)]

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::{format_ident, quote};
use syn::ext::IdentExt;
use syn::{parse_macro_input, spanned::Spanned, Fields, Index, ItemStruct, Type};

mod packed;
mod record;
mod traits;

/// Generate an inline shared record from a named-field declaration.
#[proc_macro_attribute]
pub fn record(args: TokenStream, item: TokenStream) -> TokenStream {
    let args = proc_macro2::TokenStream::from(args);
    let item = parse_macro_input!(item as ItemStruct);
    record::expand(args, item)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

/// Generate a tagged pointer owner for an enum of thin shared pointers.
#[proc_macro_derive(Packable, attributes(datarc))]
pub fn derive_packable(item: TokenStream) -> TokenStream {
    let item = parse_macro_input!(item as syn::Item);
    let result = match item {
        syn::Item::Enum(item) => packed::expand(item),
        item => Err(syn::Error::new(item.span(), "Packable requires an enum")),
    };
    result.unwrap_or_else(syn::Error::into_compile_error).into()
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
    let mut length = None;
    for attr in &field.attrs {
        if !attr.path().is_ident("datarc") {
            check_attr(attr)?;
            continue;
        }
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("len") {
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
                return Err(meta.error("expected `len = <unsigned integer type>`"));
            }
            Ok(())
        })?;
    }
    let ty = slice_element.unwrap_or(&field.ty).clone();
    if slice_element.is_some() {
        Ok((ty, Some(length.unwrap_or_else(|| syn::parse_quote!(usize)))))
    } else if length.is_some() {
        Err(syn::Error::new(
            field.span(),
            "`len` requires a sequence field `[T]`",
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

/// Generate a shared owner from a direct-field declaration.
fn generate_owner(
    item: ItemStruct,
    hash_config: Option<(TokenStream2, TokenStream2)>,
) -> syn::Result<TokenStream2> {
    let (hash_field, hash_init, hash_where, hash_compute, hash_accessor) = match hash_config {
        Some((bound, compute)) => (
            quote!(__hash: u64,),
            quote!(__hash: 0,),
            quote!(where #bound),
            compute,
            quote! {
                /// Return the field hash computed during construction.
                #[inline]
                pub fn cached_hash(&self) -> u64 { self.__datarc.get().__hash }
            },
        ),
        None => (quote!(), quote!(), quote!(), quote!(), quote!()),
    };
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
        if ["ptr_eq", "clone"]
            .iter()
            .any(|name| ident.unraw() == *name)
        {
            return Err(syn::Error::new(
                ident.span(),
                "field name conflicts with a generated sharing method",
            ));
        }
        if !hash_field.is_empty() && ident.unraw() == "cached_hash" {
            return Err(syn::Error::new(
                ident.span(),
                "field name conflicts with the generated cached_hash accessor",
            ));
        }
        if ["new", "try_new", "new_static"]
            .iter()
            .any(|reserved| ident.unraw() == *reserved)
        {
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

    let runtime = runtime_path();
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
        .map(|i| traits::fresh_ident(generics, &format!("__datarc_input_{i}")))
        .collect();
    let fresh_ident = |stem: &str| {
        let mut candidate = traits::fresh_ident(generics, stem);
        while fields.iter().any(|f| f.ident.unraw() == candidate.unraw()) {
            candidate = traits::fresh_ident(generics, &format!("{candidate}_"));
        }
        candidate
    };
    let names: Vec<_> = fields
        .iter()
        .map(|f| {
            if generics.const_params().any(|p| p.ident == f.ident) {
                fresh_ident(&format!("__datarc_arg_{}", f.ident.unraw()))
            } else {
                f.ident.clone()
            }
        })
        .collect();
    let constructor = fresh_ident("__datarc_construct");
    let allocator = fresh_ident("__datarc_allocator");
    let is_static = fresh_ident("__datarc_static");
    let other = fresh_ident("__datarc_other");
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
        #[repr(C, align(16))]
        struct #storage #generics #where_clause {
            __count: #runtime::__private::RefCount,
            #hash_field
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
            /// Return whether both owners share the same allocation.
            /// Distinct allocations can still contain equal values.
            #[inline]
            #vis fn ptr_eq(&self, #other: &Self) -> bool {
                ::std::ptr::eq(self.__datarc.as_ptr(), #other.__datarc.as_ptr())
            }

            /// Construct a record from exact-size iterable inputs.
            /// Panics if validation fails or an input iterator panics.
            #[allow(clippy::too_many_arguments)]
            #vis fn new(#(#names: #param_types),*) -> Self #hash_where {
                match Self::try_new(#(#names),*) {
                    ::std::result::Result::Ok(__value) => __value,
                    ::std::result::Result::Err(__error) => ::std::panic!("{}", __error),
                }
            }

            /// Construct a record from exact-size iterable inputs, validating
            /// reported lengths, allocation layout, and actual item counts.
            /// Iterator panics propagate after cleaning up initialized values.
            /// Allocation failure follows the standard allocator's failure behavior.
            #[allow(clippy::too_many_arguments)]
            #vis fn try_new(#(#names: #param_types),*) -> ::std::result::Result<Self, #runtime::BuildError> #hash_where {
                // SAFETY: the global allocator returns correctly sized and
                // aligned storage, which this owner may later deallocate.
                unsafe { Self::#constructor(#runtime::__private::allocate, false, #(#names,)*) }
            }

            /// Construct an immortal record in caller-provided permanent storage.
            /// Clone and Drop inspect the immortal sentinel without updating it.
            /// Neither the stored values nor the storage are destroyed on success.
            ///
            /// Panics on invalid lengths, layout, or iterator item counts. If
            /// initialization fails or unwinds, initialized values are destroyed
            /// but the supplied storage is never deallocated. A null pointer
            /// follows the standard allocator's allocation-failure behavior.
            ///
            /// # Safety
            /// The callback must return a unique, writable region satisfying the
            /// supplied Layout's size and alignment. On success, the region must
            /// remain valid for the rest of the process and must not be reused,
            /// freed, or externally modified while it contains this record.
            /// Payload references must still obey their own lifetimes.
            #[allow(clippy::too_many_arguments)]
            #vis unsafe fn new_static(
                #allocator: impl ::std::ops::FnOnce(::std::alloc::Layout) -> *mut u8,
                #(#names: #param_types),*
            ) -> Self #hash_where {
                // SAFETY: the caller guarantees permanent storage with the
                // requested layout; count mode is fixed before publication.
                match unsafe { Self::#constructor(
                    |__layout| ::std::ptr::NonNull::new(#allocator(__layout))
                        .unwrap_or_else(|| ::std::alloc::handle_alloc_error(__layout)),
                    true,
                    #(#names,)*
                ) } {
                    ::std::result::Result::Ok(__value) => __value,
                    ::std::result::Result::Err(__error) => ::std::panic!("{}", __error),
                }
            }

            // SAFETY: the allocator must supply a unique writable region with
            // the requested layout. Counted storage must be globally allocated;
            // immortal storage must remain valid permanently on success.
            #[allow(clippy::too_many_arguments)]
            unsafe fn #constructor(
                #allocator: impl ::std::ops::FnOnce(::std::alloc::Layout) -> ::std::ptr::NonNull<u8>,
                #is_static: bool,
                #(#names: #param_types),*
            ) -> ::std::result::Result<Self, #runtime::BuildError> #hash_where {
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
                let __allocation = #allocator(__layout);
                // SAFETY: header initialization cannot unwind. Install its
                // allocation guard before calling any user iterator code.
                unsafe {
                    __allocation.cast::<#storage #ty_generics>().as_ptr().write(#storage {
                        __count: #runtime::__private::RefCount::default(),
                        #hash_init
                        __lengths,
                        __values: (#(#once_inputs,)*),
                        __marker: ::std::marker::PhantomData,
                    });
                }
                // SAFETY: the header is initialized and uniquely owned here.
                let __header_guard = unsafe {
                    #runtime::__private::Allocation::new(__allocation.cast::<#storage #ty_generics>(), __layout, !#is_static)
                };
                #initialize_tails
                // Every array is initialized and every input iterator is destroyed.
                // Transfer cleanup to a guard that also destroys the arrays if hashing panics.
                let __ptr = __header_guard.into_raw();
                // SAFETY: storage is fully initialized and exclusively owned.
                // Keep cleanup independent of the refcount, including for caller-owned storage.
                let __initialized = unsafe {
                    #runtime::__private::InitializationGuard::new(__ptr, __layout, !#is_static)
                };
                let __owner = ::std::mem::ManuallyDrop::new(Self {
                    __datarc: unsafe { #runtime::__private::Shared::from_raw(__ptr) }
                });
                #hash_compute
                if #is_static {
                    // SAFETY: the owner is uniquely held and not yet published.
                    unsafe { (*__owner.__datarc.as_ptr()).__count = #runtime::__private::RefCount::new_static(); }
                }
                __initialized.disarm();
                ::std::result::Result::Ok(::std::mem::ManuallyDrop::into_inner(__owner))
            }

            #hash_accessor
            #(#accessors)*
        }

        // SAFETY: Shared transfers one strong unit; the storage header has
        // alignment at least 16 and reconstruction preserves its provenance.
        unsafe impl #impl_generics #runtime::PackedPointer for #name #ty_generics #where_clause {
            fn into_raw(self) -> ::std::ptr::NonNull<()> {
                self.__datarc.into_raw().cast()
            }

            unsafe fn from_raw(__ptr: ::std::ptr::NonNull<()>) -> Self {
                Self {
                    // SAFETY: caller transfers a live strong unit for this owner.
                    __datarc: unsafe { #runtime::__private::Shared::from_raw(__ptr.cast()) },
                }
            }
        }

        impl #impl_generics ::std::clone::Clone for #name #ty_generics #where_clause {
            fn clone(&self) -> Self { Self { __datarc: self.__datarc.clone() } }
        }
    })
}

fn runtime_path() -> TokenStream2 {
    match proc_macro_crate::crate_name("datarc") {
        Ok(proc_macro_crate::FoundCrate::Name(name)) => {
            let ident = format_ident!("{name}");
            quote!(::#ident)
        }
        // Also resolves in integration tests of the runtime package.
        _ => quote!(::datarc),
    }
}
