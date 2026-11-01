use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::{spanned::Spanned, Fields, ItemEnum};

pub(super) fn expand(item: ItemEnum) -> syn::Result<TokenStream> {
    let runtime = super::runtime_path();
    let name = &item.ident;
    let vis = &item.vis;
    let mut packed = None;
    for attr in item.attrs.iter().filter(|a| a.path().is_ident("datarc")) {
        attr.parse_nested_meta(|meta| {
            if !meta.path.is_ident("packed") {
                return Err(meta.error("expected `packed = PackedName`"));
            }
            if packed.is_some() {
                return Err(meta.error("specify the packed name only once"));
            }
            packed = Some(meta.value()?.parse::<syn::Ident>()?);
            Ok(())
        })?;
    }
    let packed = packed.unwrap_or_else(|| format_ident!("{}Packed", name));
    if packed == *name {
        return Err(syn::Error::new(
            packed.span(),
            "enum and packed type must have different names",
        ));
    }
    if item.variants.is_empty() || item.variants.len() > 16 {
        return Err(syn::Error::new(
            item.span(),
            "Packable requires between 1 and 16 variants",
        ));
    }
    // Self would refer to the packed type in copied bounds and field types.
    struct FindSelf(Option<proc_macro2::Span>);
    impl<'ast> syn::visit::Visit<'ast> for FindSelf {
        fn visit_ident(&mut self, ident: &'ast syn::Ident) {
            if ident == "Self" {
                self.0 = Some(ident.span());
            }
        }
    }
    let mut finder = FindSelf(None);
    syn::visit::Visit::visit_generics(&mut finder, &item.generics);
    let mut variants = Vec::new();
    let mut types = Vec::new();
    for variant in &item.variants {
        for attr in variant
            .attrs
            .iter()
            .chain(variant.fields.iter().flat_map(|f| &f.attrs))
        {
            if attr.path().is_ident("cfg") || attr.path().is_ident("cfg_attr") {
                return Err(syn::Error::new(
                    attr.span(),
                    "conditional variants and fields are not supported by Packable",
                ));
            }
        }
        let Fields::Unnamed(fields) = &variant.fields else {
            return Err(syn::Error::new(
                variant.span(),
                "Packable requires single-item tuple variants",
            ));
        };
        if fields.unnamed.len() != 1 {
            return Err(syn::Error::new(
                fields.span(),
                "Packable requires single-item tuple variants",
            ));
        }
        let ty = &fields.unnamed[0].ty;
        syn::visit::Visit::visit_type(&mut finder, ty);
        variants.push(&variant.ident);
        types.push(ty);
    }
    if let Some(span) = finder.0 {
        return Err(syn::Error::new(
            span,
            "use the enum's name instead of `Self` in its declaration",
        ));
    }
    let mut generics = item.generics.clone();
    for ty in &types {
        generics
            .make_where_clause()
            .predicates
            .push(syn::parse_quote!(#ty: #runtime::PackedPointer));
    }
    let (imp, args, bounds) = generics.split_for_impl();
    let mut result_name = "__DatarcResult".to_owned();
    while generics.params.iter().any(|param| match param {
        syn::GenericParam::Type(p) => p.ident == result_name,
        syn::GenericParam::Const(p) => p.ident == result_name,
        _ => false,
    }) {
        result_name.push('_');
    }
    let result_name = format_ident!("{result_name}");
    let tags: Vec<_> = (0..variants.len()).collect();
    let mut send_generics = generics.clone();
    send_generics
        .make_where_clause()
        .predicates
        .push(syn::parse_quote!(#name #args: ::std::marker::Send));
    let (send_imp, _, send_bounds) = send_generics.split_for_impl();
    let mut sync_generics = generics.clone();
    sync_generics
        .make_where_clause()
        .predicates
        .push(syn::parse_quote!(#name #args: ::std::marker::Sync));
    let (sync_imp, _, sync_bounds) = sync_generics.split_for_impl();
    Ok(quote! {
        impl #imp #runtime::Packable for #name #args #bounds {
            type Packed = #packed #args;
        }

        /// A pointer-sized owning representation with a four-bit variant tag.
        #[repr(transparent)]
        #vis struct #packed #generics #bounds {
            __ptr: ::std::ptr::NonNull<()>,
            __marker: ::std::marker::PhantomData<#name #args>,
        }

        // SAFETY: the tagged pointer owns exactly the value described by the
        // original enum. Its thread-safety and drop checking follow that enum.
        unsafe impl #send_imp ::std::marker::Send for #packed #args #send_bounds {}
        unsafe impl #sync_imp ::std::marker::Sync for #packed #args #sync_bounds {}

        impl #imp #packed #args #bounds {
            /// Consume the packed owner, restoring its original enum.
            #vis fn into_enum(self) -> #name #args {
                let __this = ::std::mem::ManuallyDrop::new(self);
                // SAFETY: the packed owner is disarmed and transfers its unit.
                unsafe { __this.__datarc_unpack() }
            }

            /// Inspect the original enum by reference without changing counts.
            /// The borrowed enum cannot outlive this call.
            #vis fn with_enum<#result_name>(
                &self,
                __f: impl ::std::ops::FnOnce(&#name #args) -> #result_name,
            ) -> #result_name {
                // SAFETY: this temporary is never dropped or moved out; shared
                // access is valid while self retains its ownership unit.
                let __enum = ::std::mem::ManuallyDrop::new(unsafe { self.__datarc_unpack() });
                __f(&__enum)
            }

            /// Clone the stored pointer into an ordinary enum.
            #vis fn to_enum(&self) -> #name #args {
                self.with_enum(|__enum| match __enum {
                    #(#name::#variants(__value) => #name::#variants(
                        <#types as ::std::clone::Clone>::clone(__value)
                    ),)*
                })
            }

            // SAFETY: caller must transfer ownership or suppress destruction of
            // the restored value, and must not allow it to escape as a new unit.
            unsafe fn __datarc_unpack(&self) -> #name #args {
                let __tag = self.__ptr.addr().get() & 15;
                let __ptr = self.__ptr.map_addr(|__addr| {
                    // SAFETY: removing tag bits restores a non-null aligned pointer.
                    unsafe { ::std::num::NonZeroUsize::new_unchecked(__addr.get() & !15) }
                });
                match __tag {
                    #(#tags => #name::#variants(
                        // SAFETY: the tag selects the pointer's original type.
                        unsafe { <#types as #runtime::PackedPointer>::from_raw(__ptr) }
                    ),)*
                    _ => ::std::unreachable!("invalid packed enum tag"),
                }
            }
        }

        impl #imp ::std::convert::From<#name #args> for #packed #args #bounds {
            fn from(__enum: #name #args) -> Self {
                let __ptr = match __enum {
                    #(#name::#variants(__value) => {
                        let __ptr = <#types as #runtime::PackedPointer>::into_raw(__value);
                        __ptr.map_addr(|__addr| {
                            // SAFETY: OR-ing four tag bits cannot make a non-null
                            // pointer zero. The trait guarantees zero low bits.
                            unsafe { ::std::num::NonZeroUsize::new_unchecked(__addr.get() | #tags) }
                        })
                    },)*
                };
                Self { __ptr, __marker: ::std::marker::PhantomData }
            }
        }

        impl #imp ::std::convert::From<&#name #args> for #packed #args #bounds {
            fn from(__enum: &#name #args) -> Self {
                Self::from(match __enum {
                    #(#name::#variants(__value) => #name::#variants(
                        <#types as ::std::clone::Clone>::clone(__value)
                    ),)*
                })
            }
        }

        impl #imp ::std::convert::From<#packed #args> for #name #args #bounds {
            fn from(__packed: #packed #args) -> Self { __packed.into_enum() }
        }

        impl #imp ::std::convert::From<&#packed #args> for #name #args #bounds {
            fn from(__packed: &#packed #args) -> Self { __packed.to_enum() }
        }

        impl #imp ::std::clone::Clone for #packed #args #bounds {
            fn clone(&self) -> Self { Self::from(self.to_enum()) }
        }

        impl #imp ::std::ops::Drop for #packed #args #bounds {
            fn drop(&mut self) {
                // SAFETY: destruction transfers the only unit owned by self.
                ::std::mem::drop(unsafe { self.__datarc_unpack() });
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Checks that enum expansion rejects unsupported variant shapes, Self payloads,
    /// conditional variants, name collisions, and more than 16 variants.
    #[test]
    fn rejects_unsupported_enums() {
        for input in [
            quote!(
                enum E {}
            ),
            quote!(
                enum E {
                    A,
                }
            ),
            quote!(
                enum E {
                    A { x: P },
                }
            ),
            quote!(
                enum E {
                    A(P, P),
                }
            ),
            quote!(
                enum E {
                    A(Self),
                }
            ),
            quote!(
                enum E {
                    #[cfg(any())]
                    A(P),
                }
            ),
            quote!(
                #[datarc(packed = E)]
                enum E {
                    A(P),
                }
            ),
        ] {
            assert!(expand(syn::parse2(input).unwrap()).is_err());
        }
        let variants = (0..17).map(|i| format_ident!("V{i}"));
        assert!(expand(syn::parse2(quote!(enum E { #(#variants(P),)* })).unwrap()).is_err());
    }
}
