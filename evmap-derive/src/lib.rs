use proc_macro2::{Ident, TokenStream};
use quote::{format_ident, quote, quote_spanned};
use syn::spanned::Spanned;
use syn::{parse_macro_input, parse_quote, Data, DeriveInput, Fields, GenericParam, Generics, Index};

#[proc_macro_derive(ShallowCopy)]
pub fn derive_shallow_copy(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    let name = input.ident;
    let generics = add_trait_bounds(input.generics);
    let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();
    let copy = fields(&input.data, &name);
    proc_macro::TokenStream::from(quote! {
        impl #impl_generics evmap::shallow_copy::ShallowCopy for #name #ty_generics #where_clause {
            unsafe fn shallow_copy(&self) -> std::mem::ManuallyDrop<Self> {
                #copy
            }
        }
    })
}

fn add_trait_bounds(mut generics: Generics) -> Generics {
    for param in &mut generics.params {
        if let GenericParam::Type(ref mut type_param) = *param {
            type_param.bounds.push(parse_quote!(evmap::shallow_copy::ShallowCopy));
        }
    }
    generics
}

fn fields(data: &Data, type_name: &Ident) -> TokenStream {
    match data {
        Data::Struct(data) => {
            match &data.fields {
                Fields::Named(fields) => {
                    let recurse = fields.named.iter().map(|f| {
                        let name = &f.ident;
                        quote_spanned! {f.span()=>
                            #name: std::mem::ManuallyDrop::into_inner(evmap::shallow_copy::ShallowCopy::shallow_copy(&self.#name))
                        }
                    });
                    quote! {
                        std::mem::ManuallyDrop::new(Self { #(#recurse,)* })
                    }
                }
                Fields::Unnamed(fields) => {
                    let recurse = fields.unnamed.iter().enumerate().map(|(i, f)| {
                        let index = Index::from(i);
                        quote_spanned! {f.span()=>
                            std::mem::ManuallyDrop::into_inner(evmap::shallow_copy::ShallowCopy::shallow_copy(&self.#index))
                        }
                    });
                    quote! {
                        std::mem::ManuallyDrop::new((#(#recurse,)*))
                    }
                }
                Fields::Unit => {
                    quote!(())
                }
            }
        }
        Data::Enum(data) => {
            let recurse = data.variants.iter().map(|f| {
                let field_names = f.fields.iter().enumerate().map(|(i, field)| {
                    let ident = format_ident!("x{}", i);
                    quote_spanned! {
                        field.span()=> #ident
                    }
                });
                let fields = f.fields.iter().enumerate().map(|(i, field)| {
                    let ident = format_ident!("x{}", i);
                    quote_spanned! {field.span()=>
                       std::mem::ManuallyDrop::into_inner(evmap::shallow_copy::ShallowCopy::shallow_copy(#ident))
                    }
                });
                let name = &f.ident;
                quote_spanned! {f.span()=>
                    #type_name::#name(#(#field_names,)*) => std::mem::ManuallyDrop::new(#type_name::#name(#(#fields,)*))
                }
            });
            quote! {
                match self {
                    #(#recurse,)*
                }
            }
        }
        Data::Union(_) => unimplemented!("Unions are not supported"),
    }
}
